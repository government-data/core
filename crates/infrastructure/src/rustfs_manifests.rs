//! A cut's files in its tenant's warehouse bucket (decision 0035): each is
//! written once, with credentials issued for that write by STS and limited
//! by the caller's session policy (`PutObject` under `manifests/` only), and
//! `If-None-Match: *`, so RustFS itself refuses to overwrite one
//! (`tools/verify/lake_manifests.py`).

use std::time::Duration;

use chrono::Utc;

use crate::rustfs_sts::StsIssuer;
use sigv4::{encode, payload_hash};

/// Writes manifests through the platform's STS issuer.
#[derive(Debug, Clone)]
pub struct ManifestWriter {
    issuer: StsIssuer,
}

impl ManifestWriter {
    pub fn new(issuer: StsIssuer) -> Self {
        Self { issuer }
    }

    /// Writes `body` at `bucket/key` unless an object is there; `Ok(false)`
    /// if one was. `policy` is the session policy for this write.
    pub async fn put_once(
        &self,
        bucket: &str,
        key: &str,
        body: Vec<u8>,
        policy: &str,
    ) -> Result<bool, String> {
        let creds = self
            .issuer
            .assume_role("manifest", policy, Duration::from_secs(900))
            .await
            .map_err(|e| e.to_string())?;
        let path = format!("/{}/{}", encode(bucket, false), encode(key, true));
        let url = format!("{}{path}", self.issuer.endpoint());
        let host = sigv4::host(&url).ok_or_else(|| format!("bad endpoint {url}"))?;
        let hash = payload_hash(&body);
        let headers = [
            ("content-type", "application/json"),
            ("host", host.as_str()),
            ("if-none-match", "*"),
            ("x-amz-content-sha256", hash.as_str()),
            ("x-amz-security-token", creds.session_token.as_str()),
        ];
        let signed = sigv4::sign(
            "s3",
            &creds.access_key,
            &creds.secret_key,
            "PUT",
            &path,
            &headers,
            Utc::now(),
        );
        let mut req = self.issuer.http().put(&url).body(body);
        for (name, value) in headers.iter().filter(|(n, _)| *n != "host") {
            req = req.header(*name, *value);
        }
        let res = req
            .header("x-amz-date", &signed.date)
            .header("authorization", &signed.authorization)
            .send()
            .await
            .map_err(|e| format!("manifest write: {e}"))?;
        match res.status().as_u16() {
            200 => Ok(true),
            412 => Ok(false),
            status => Err(format!("manifest write refused ({status})")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn a_put_is_signed_as_botocore_signs_it() {
        // botocore's S3SigV4Auth("s3", "us-east-1") on the same request.
        let path = format!(
            "/qd-tenant-a/{}",
            encode("manifests/loans/2021-03-10/evaluate/j.json", true)
        );
        let hash = payload_hash(b"{}");
        let now = Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap();
        let signed = sigv4::sign(
            "s3",
            "AKID",
            "SECRET",
            "PUT",
            &path,
            &[
                ("content-type", "application/json"),
                ("host", "lake.example"),
                ("if-none-match", "*"),
                ("x-amz-content-sha256", &hash),
                ("x-amz-security-token", "TOKEN"),
            ],
            now,
        );
        assert_eq!(
            signed.authorization,
            "AWS4-HMAC-SHA256 Credential=AKID/20261031/us-east-1/s3/aws4_request, \
             SignedHeaders=content-type;host;if-none-match;x-amz-content-sha256;x-amz-date;\
             x-amz-security-token, \
             Signature=7627a6e111224185756b4daff5357a350b973b5d145c484a792d066f7b9a7cec"
        );
    }
}
