//! A lake's STS (decisions 0030 and 0078): `AssumeRole` as the platform's
//! issuer, narrowed by a session policy, for credentials that live minutes
//! and reach what the policy says. Where STS answers, the role asked for
//! and the shortest credentials are the lake's declared properties
//! ([`LakeProperties`]); the issuer's own policy is the ceiling, the
//! caller's session policy what each job may do.

use std::time::Duration;

use chrono::{DateTime, Utc};

use application::lake::LakeProperties;

use crate::lake_http::LakeClient;
use sigv4::{encode, payload_hash};

/// Temporary credentials. The secret and the token never show in `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct TemporaryCredentials {
    pub access_key: String,
    pub secret_key: String,
    pub session_token: String,
    pub expires_at: DateTime<Utc>,
}

impl std::fmt::Debug for TemporaryCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TemporaryCredentials")
            .field("access_key", &self.access_key)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum StsError {
    Http(reqwest::Error),
    Refused { status: u16, code: String },
    Answer(String),
}

impl std::fmt::Display for StsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "sts request: {e}"),
            Self::Refused { status, code } => write!(f, "sts refused ({status}): {code}"),
            Self::Answer(e) => write!(f, "sts answer: {e}"),
        }
    }
}

impl std::error::Error for StsError {}

impl From<reqwest::Error> for StsError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}

/// The issuer: the lake's declared properties and the issuer's keys.
#[derive(Clone)]
pub struct StsIssuer {
    http: LakeClient,
    properties: LakeProperties,
    endpoint: String,
    access_key: String,
    secret_key: String,
}

impl std::fmt::Debug for StsIssuer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StsIssuer")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl StsIssuer {
    pub fn new(
        http: impl Into<LakeClient>,
        properties: LakeProperties,
        access_key: &str,
        secret_key: &str,
    ) -> Self {
        Self {
            http: http.into(),
            endpoint: properties.s3_endpoint.trim_end_matches('/').to_string(),
            properties,
            access_key: access_key.into(),
            secret_key: secret_key.into(),
        }
    }

    /// The lake's S3 endpoint, where STS answers too.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// What the lake declares.
    pub fn properties(&self) -> &LakeProperties {
        &self.properties
    }

    /// Its HTTP client, which trusts the lake's certificate and presents
    /// the machine's, the one in use now.
    pub fn http(&self) -> reqwest::Client {
        self.http.get()
    }

    /// Credentials for `duration` (never below the lake's minimum), within
    /// `policy` (a JSON IAM policy); `session` names them in the lake's logs.
    pub async fn assume_role(
        &self,
        session: &str,
        policy: &str,
        duration: Duration,
    ) -> Result<TemporaryCredentials, StsError> {
        let seconds = self
            .properties
            .credentials_duration(duration)
            .as_secs()
            .to_string();
        let mut pairs = vec![
            ("Action", "AssumeRole"),
            ("DurationSeconds", seconds.as_str()),
            ("Policy", policy),
        ];
        if let Some(role) = &self.properties.sts_role {
            pairs.push(("RoleArn", role.as_str()));
        }
        pairs.extend([("RoleSessionName", session), ("Version", "2011-06-15")]);
        let body = form(&pairs);
        let url = format!("{}/", self.endpoint);
        let host =
            sigv4::host(&url).ok_or_else(|| StsError::Answer(format!("bad endpoint {url}")))?;
        let signed = sign(&self.access_key, &self.secret_key, &host, &body, Utc::now());
        let res = self
            .http
            .get()
            .post(&url)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("x-amz-content-sha256", &signed.payload_hash)
            .header("x-amz-date", &signed.date)
            .header("authorization", &signed.authorization)
            .body(body)
            .send()
            .await?;
        let status = res.status().as_u16();
        let text = res.text().await?;
        if status != 200 {
            return Err(StsError::Refused {
                status,
                code: tag(&text, "Code").unwrap_or_default(),
            });
        }
        credentials(&text)
    }
}

fn tag(xml: &str, name: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let node = doc.descendants().find(|n| n.has_tag_name(name))?;
    node.text().map(str::to_string)
}

fn credentials(xml: &str) -> Result<TemporaryCredentials, StsError> {
    let get = |n: &str| tag(xml, n).ok_or_else(|| StsError::Answer(format!("no {n}")));
    let expires_at = DateTime::parse_from_rfc3339(&get("Expiration")?)
        .map_err(|e| StsError::Answer(format!("Expiration: {e}")))?
        .with_timezone(&Utc);
    Ok(TemporaryCredentials {
        access_key: get("AccessKeyId")?,
        secret_key: get("SecretAccessKey")?,
        session_token: get("SessionToken")?,
        expires_at,
    })
}

/// `application/x-www-form-urlencoded`, as SigV4 wants it encoded.
fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k, false), encode(v, false)))
        .collect::<Vec<_>>()
        .join("&")
}

struct Signed {
    date: String,
    payload_hash: String,
    authorization: String,
}

/// SigV4 for a `POST /` to STS (service `sts`), signing the content type,
/// the host, the payload's hash and the date.
fn sign(access: &str, secret: &str, host: &str, body: &str, now: DateTime<Utc>) -> Signed {
    let payload_hash = payload_hash(body.as_bytes());
    let signed = sigv4::sign(
        "sts",
        access,
        secret,
        "POST",
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("host", host),
            ("x-amz-content-sha256", &payload_hash),
        ],
        now,
    );
    Signed {
        date: signed.date,
        payload_hash,
        authorization: signed.authorization,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn signs_as_botocore_does() {
        // botocore's SigV4Auth("sts", "us-east-1") on the same request.
        let body = form(&[
            ("Action", "AssumeRole"),
            ("DurationSeconds", "900"),
            ("Policy", "{}"),
            ("RoleArn", "arn:aws:iam::000000000000:role/qd-job"),
            ("RoleSessionName", "job-1"),
            ("Version", "2011-06-15"),
        ]);
        let now = Utc.with_ymd_and_hms(2026, 10, 30, 12, 0, 0).unwrap();
        let s = sign("AKID", "SECRET", "lake.example", &body, now);
        assert_eq!(s.date, "20261030T120000Z");
        assert_eq!(
            s.authorization,
            "AWS4-HMAC-SHA256 Credential=AKID/20261030/us-east-1/sts/aws4_request, \
             SignedHeaders=content-type;host;x-amz-content-sha256;x-amz-date, \
             Signature=40b1aedce9f0b25e18e41b697f55ceb26a55ed88bc10403de23b582989fa7bb4"
        );
    }

    #[test]
    fn reads_the_credentials_and_hides_the_secret() {
        let xml = r#"<AssumeRoleResponse xmlns="https://sts.amazonaws.com/doc/2011-06-15/">
          <AssumeRoleResult><Credentials>
            <AccessKeyId>TMPKEY</AccessKeyId><SecretAccessKey>TMPSECRET</SecretAccessKey>
            <SessionToken>TMPTOKEN</SessionToken><Expiration>2026-10-30T12:15:00Z</Expiration>
          </Credentials></AssumeRoleResult></AssumeRoleResponse>"#;
        let c = credentials(xml).unwrap();
        assert_eq!(
            (c.access_key.as_str(), c.session_token.as_str()),
            ("TMPKEY", "TMPTOKEN")
        );
        assert_eq!(
            c.expires_at,
            Utc.with_ymd_and_hms(2026, 10, 30, 12, 15, 0).unwrap()
        );
        let shown = format!("{c:?}");
        assert!(!shown.contains("TMPSECRET") && !shown.contains("TMPTOKEN"));
        assert_eq!(
            tag(
                "<ErrorResponse><Error><Code>AccessDenied</Code></Error></ErrorResponse>",
                "Code"
            )
            .as_deref(),
            Some("AccessDenied")
        );
    }
}
