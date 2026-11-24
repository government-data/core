//! A lake's admin API (decision 0028's `LakeAdmin`; 0066 step 11): RustFS's
//! `/rustfs/admin/v3/…`, signed with the platform admin keys a person pasted
//! for the lake, which this adapter alone reads, through the lake's gate
//! with the application VM's certificate ([`LakeClient`]). Verified on
//! 2026-11-13 with RustFS 1.0.5 (`tools/verify/rustfs_replication_gate.sh`):
//! `info` answers its drives under `info.backend`; refused keys are a 403
//! with `SignatureDoesNotMatch` or `InvalidAccessKeyId`. No key, secret or
//! signature is logged or answered.

use std::sync::Arc;
use std::time::Duration;

use application::lake::LakeProbe;
use async_trait::async_trait;
use auth_gateway::{Lake, LakeAdminKeys};
use chrono::{DateTime, Utc};
use domain::RepositoryError;

use crate::lake_http::LakeClient;

/// The port a lake node's gate listens on: the replica's compose file
/// publishes it as `RUSTFS_PORT`, 9000 unless a person changed it.
pub const LAKE_GATE_PORT: u16 = 9000;

const INFO: &str = "/rustfs/admin/v3/info";

/// What RustFS answers when it refuses the keys themselves.
const REFUSED: [&str; 3] = [
    "SignatureDoesNotMatch",
    "InvalidAccessKeyId",
    "AccessDenied",
];

/// Where the adapter reads a lake's admin keys, the secret opened. Only
/// this adapter is given it.
#[async_trait]
pub trait AdminKeySource: Send + Sync {
    async fn open(&self, lake: &Lake) -> Result<Option<LakeAdminKeys>, RepositoryError>;
}

/// The lakes' admin API over HTTPS, a node reached by its name.
#[derive(Clone)]
pub struct RustfsLakeAdmin {
    pub(crate) http: LakeClient,
    pub(crate) keys: Arc<dyn AdminKeySource>,
    scheme: &'static str,
    port: u16,
}

impl std::fmt::Debug for RustfsLakeAdmin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RustfsLakeAdmin")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl RustfsLakeAdmin {
    /// Over HTTPS on [`LAKE_GATE_PORT`].
    pub fn new(http: impl Into<LakeClient>, keys: Arc<dyn AdminKeySource>) -> Self {
        Self {
            http: http.into(),
            keys,
            scheme: "https",
            port: LAKE_GATE_PORT,
        }
    }

    /// From the lake's HTTP client of the environment (`LAKE_CA_FILE`,
    /// `LAKE_CLIENT_CERT_FILE`), as the STS issuer's.
    pub fn from_env(keys: Arc<dyn AdminKeySource>) -> Result<Self, String> {
        Ok(Self::new(
            LakeClient::from_env(Duration::from_secs(10))?,
            keys,
        ))
    }

    /// Where `node` answers: a base URL as it is (the main lake's,
    /// `LAKE_ENDPOINT`), or a node's name on the gate's port.
    pub(crate) fn base(&self, node: &str) -> String {
        if node.contains("://") {
            node.trim_end_matches('/').to_string()
        } else {
            format!("{}://{node}:{}", self.scheme, self.port)
        }
    }

    /// Another scheme and port, for tests.
    pub fn at(mut self, scheme: &'static str, port: u16) -> Self {
        self.scheme = scheme;
        self.port = port;
        self
    }
}

impl RustfsLakeAdmin {
    /// Asks `node` of `lake` for its `info`: a node's name, reached on its
    /// gate's port, or the lake's base URL (the main lake's).
    pub async fn probe(&self, lake: &Lake, node: &str) -> Result<LakeProbe, RepositoryError> {
        let Some(keys) = self.keys.open(lake).await? else {
            return Ok(LakeProbe::NoKeys);
        };
        let url = format!("{}{INFO}", self.base(node));
        let Some(host) = sigv4::host(&url) else {
            let detail = format!("not a node's name: {node}");
            return Ok(LakeProbe::Unreachable { detail });
        };
        let signed = sign_get(&keys, &host, INFO, Utc::now());
        let sent = self
            .http
            .get()
            .get(&url)
            .header("content-length", "0")
            .header("x-amz-content-sha256", &signed.payload_hash)
            .header("x-amz-date", &signed.date)
            .header("authorization", &signed.authorization)
            .send()
            .await;
        let res = match sent {
            Ok(res) => res,
            Err(e) => return Ok(LakeProbe::Unreachable { detail: chain(&e) }),
        };
        let status = res.status().as_u16();
        let text = match res.text().await {
            Ok(text) => text,
            Err(e) => return Ok(LakeProbe::Unreachable { detail: chain(&e) }),
        };
        Ok(answer(status, &text))
    }
}

/// What `info`'s answer says.
fn answer(status: u16, text: &str) -> LakeProbe {
    let code = code(text);
    if status == 403 && REFUSED.contains(&code.as_str()) {
        return LakeProbe::KeysRefused { code };
    }
    if status != 200 {
        let detail = format!("HTTP {status} {code}").trim_end().to_string();
        return LakeProbe::Unreachable { detail };
    }
    let body: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    let backend = &body["info"]["backend"];
    let count = |name: &str| backend[name].as_u64().map(|n| n as u32);
    match (count("onlineDisks"), count("offlineDisks")) {
        (Some(drives_online), Some(drives_offline)) => LakeProbe::Answered {
            drives_online,
            drives_offline,
        },
        _ => LakeProbe::Unreachable {
            detail: "info without its drives".into(),
        },
    }
}

/// An S3 error's `<Code>`, or nothing.
pub(crate) fn code(xml: &str) -> String {
    roxmltree::Document::parse(xml)
        .ok()
        .and_then(|doc| {
            let node = doc.descendants().find(|n| n.has_tag_name("Code"))?;
            node.text().map(str::to_string)
        })
        .unwrap_or_default()
}

/// An error and its causes, in a few words: the address and what failed
/// (a refused connection, a certificate, a timeout), never a header.
pub(crate) fn chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut words = e.to_string();
    let mut cause = e.source();
    while let Some(c) = cause {
        words.push_str(": ");
        words.push_str(&c.to_string());
        cause = c.source();
    }
    words.chars().take(300).collect()
}

struct Signed {
    date: String,
    payload_hash: String,
    authorization: String,
}

/// SigV4 for a `GET` with no body to the admin API (service `s3`), signing
/// the length, the host, the payload's hash and the date, as RustFS checks
/// them (0028's verification).
fn sign_get(keys: &LakeAdminKeys, host: &str, path: &str, now: DateTime<Utc>) -> Signed {
    let payload_hash = sigv4::payload_hash(b"");
    let signed = sigv4::sign(
        "s3",
        &keys.access_key,
        &keys.secret_key,
        "GET",
        path,
        &[
            ("content-length", "0"),
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
        // botocore's S3SigV4Auth("s3", "us-east-1") on the same request.
        let keys = LakeAdminKeys {
            access_key: "qd-platform-admin".into(),
            secret_key: "ab".repeat(32),
        };
        let now = Utc.with_ymd_and_hms(2026, 11, 13, 12, 0, 0).unwrap();
        let s = sign_get(&keys, "replica-1:9000", INFO, now);
        assert_eq!(s.date, "20261113T120000Z");
        assert_eq!(
            s.authorization,
            "AWS4-HMAC-SHA256 Credential=qd-platform-admin/20261113/us-east-1/s3/aws4_request, \
             SignedHeaders=content-length;host;x-amz-content-sha256;x-amz-date, \
             Signature=296d0f13761c77e8f2d635ebc40afaee8b0f3309e0504c996589f4e317b7c6cf"
        );
    }

    #[test]
    fn reads_the_drives_and_the_refusals() {
        let info =
            r#"{"info":{"backend":{"backendType":"Erasure","onlineDisks":2,"offlineDisks":1}}}"#;
        let drives = LakeProbe::Answered {
            drives_online: 2,
            drives_offline: 1,
        };
        assert_eq!(answer(200, info), drives);
        let error = |c: &str| format!("<Error><Code>{c}</Code><Message>m</Message></Error>");
        for c in REFUSED {
            let refused = LakeProbe::KeysRefused { code: c.into() };
            assert_eq!(answer(403, &error(c)), refused);
        }
        let other = |detail: &str| LakeProbe::Unreachable {
            detail: detail.into(),
        };
        assert_eq!(
            answer(403, &error("Forbidden")),
            other("HTTP 403 Forbidden")
        );
        assert_eq!(answer(502, "Bad Gateway"), other("HTTP 502"));
        assert_eq!(answer(200, "{}"), other("info without its drives"));
    }
}
