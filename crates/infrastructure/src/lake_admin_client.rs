//! The core's client of a lake's adapter over `lake-admin/1` (decision
//! 0078, step 6): `application::lake::LakeAdmin` at the wire, HTTPS with
//! the core server's machine certificate, trusting the installation's CA
//! alone and accepting only an adapter's certificate (role `lake`). A call
//! is tried again only when the adapter says it may be (`retryable`) or
//! did not answer. No body is logged: neither a key, a policy nor an
//! answer of `/credentials`.

use std::time::Duration;

use application::lake::{
    LakeAdmin, LakeCredentialKind, LakeError, LakeGrant, LakeHealth, LakeKeys, ManagedPolicy,
    ReplicaTarget, ReplicationState, RetentionRules, SessionPolicy,
};
use async_trait::async_trait;
use contracts::lake_admin::{self as wire, paths};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// The role an adapter's certificate carries.
pub const ADAPTER_ROLE: &str = "lake";

/// How many times a call is made at most.
const ATTEMPTS: u32 = 3;

/// A client of one adapter.
#[derive(Debug, Clone)]
pub struct LakeAdminClient {
    base: String,
    http: reqwest::Client,
    pause: Duration,
}

impl LakeAdminClient {
    /// The adapter at `base` (`https://host:port`), trusting `ca_pem`
    /// alone, presenting `identity_pem` (the core server's certificate,
    /// then its key).
    pub fn new(base: &str, ca_pem: &str, identity_pem: &str) -> Result<Self, String> {
        let ca = reqwest::Certificate::from_pem(ca_pem.as_bytes()).map_err(|e| e.to_string())?;
        let identity =
            reqwest::Identity::from_pem(identity_pem.as_bytes()).map_err(|e| e.to_string())?;
        let http = reqwest::Client::builder()
            .use_rustls_tls()
            .tls_built_in_root_certs(false)
            .add_root_certificate(ca)
            .identity(identity)
            .tls_info(true)
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            http,
            pause: Duration::from_millis(500),
        })
    }

    /// The same client, pausing `pause` between attempts (tests).
    pub fn pausing(mut self, pause: Duration) -> Self {
        self.pause = pause;
        self
    }

    /// `method path` with `body`, tried again while the error may pass;
    /// the answer's body, `Null` when empty.
    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, LakeError> {
        let mut attempt = 1;
        loop {
            match self.once(method, path, body.as_ref()).await {
                Err(e) if e.retryable() && attempt < ATTEMPTS => {
                    tracing::warn!(method, path, attempt, error = %e, "lake-admin: trying again");
                    tokio::time::sleep(self.pause * attempt).await;
                    attempt += 1;
                }
                other => return other,
            }
        }
    }

    async fn once(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, LakeError> {
        let what = format!("{method} {path}");
        let verb = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|e| LakeError::Invalid(format!("{what}: {e}")))?;
        let mut request = self.http.request(verb, format!("{}{path}", self.base));
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|e| LakeError::Unavailable(format!("{what}: {}", short(&e))))?;
        if !is_adapter(&response) {
            return Err(LakeError::Refused(format!(
                "{what}: not an adapter's certificate"
            )));
        }
        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|e| LakeError::Unavailable(format!("{what}: {}", short(&e))))?;
        answer(&what, status, &text)
    }

    async fn typed<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<T, LakeError> {
        let value = self.call(method, path, body).await?;
        serde_json::from_value(value).map_err(|e| {
            LakeError::Lake(format!(
                "{method} {path}: an answer not of the protocol: {e}"
            ))
        })
    }
}

/// What an answer of `status` with `text` means: the body on 2xx, else
/// the protocol's error (or `lake` if the body is not its shape).
fn answer(what: &str, status: u16, text: &str) -> Result<Value, LakeError> {
    if (200..300).contains(&status) {
        return if text.trim().is_empty() {
            Ok(Value::Null)
        } else {
            serde_json::from_str(text).map_err(|_| LakeError::Lake(format!("{what}: not JSON")))
        };
    }
    match serde_json::from_str::<wire::ErrorBody>(text) {
        Ok(body) => Err(LakeError::from(body)),
        Err(_) if status == 503 => Err(LakeError::Unavailable(format!("{what}: {status}"))),
        Err(_) => Err(LakeError::Lake(format!("{what}: {status}"))),
    }
}

/// Whether the peer presented a certificate of role [`ADAPTER_ROLE`].
fn is_adapter(response: &reqwest::Response) -> bool {
    let Some(info) = response.extensions().get::<reqwest::tls::TlsInfo>() else {
        return false;
    };
    let Some(der) = info.peer_certificate() else {
        return false;
    };
    let Ok((_, cert)) = x509_parser::parse_x509_certificate(der) else {
        return false;
    };
    let role = cert.subject().iter_organizational_unit().next();
    role.and_then(|r| r.as_str().ok()) == Some(ADAPTER_ROLE)
}

/// An error's text without its source chain's details (no URL query).
fn short(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".into()
    } else if e.is_connect() {
        "no connection".into()
    } else {
        "the request failed".into()
    }
}

fn to_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

#[async_trait]
impl LakeAdmin for LakeAdminClient {
    async fn ensure_warehouse(&self, name: &str, rules: &RetentionRules) -> Result<(), LakeError> {
        let body = to_value(&wire::RetentionRules::from(rules));
        self.call("PUT", &paths::warehouse(name), Some(body))
            .await
            .map(|_| ())
    }

    async fn replicate(
        &self,
        name: &str,
        replica: &ReplicaTarget,
    ) -> Result<ReplicationState, LakeError> {
        let body = to_value(&wire::ReplicaTarget::from(replica));
        let state: wire::ReplicationState = self
            .typed("PUT", &paths::replication(name), Some(body))
            .await?;
        Ok(state.into())
    }

    async fn replication(&self, name: &str) -> Result<ReplicationState, LakeError> {
        let state: wire::ReplicationState =
            self.typed("GET", &paths::replication(name), None).await?;
        Ok(state.into())
    }

    async fn set_managed_policies(&self, policies: &[ManagedPolicy]) -> Result<(), LakeError> {
        let body = to_value(&wire::ManagedPolicies {
            policies: policies.iter().map(Into::into).collect(),
        });
        self.call("PUT", &paths::managed_policies(), Some(body))
            .await
            .map(|_| ())
    }

    async fn session_policy(&self, grant: &LakeGrant) -> Result<SessionPolicy, LakeError> {
        let policy: wire::SessionPolicy = self
            .typed("POST", &paths::session_policy(), Some(to_value(grant)))
            .await?;
        Ok(policy.into())
    }

    async fn ensure_credentials(
        &self,
        kind: LakeCredentialKind,
        held: Option<&LakeKeys>,
    ) -> Result<LakeKeys, LakeError> {
        let request = wire::CredentialsRequest {
            held: held.map(wire::Credentials::from),
            with: None,
        };
        let path = paths::credentials(wire::CredentialKind::from(kind));
        let keys: wire::Credentials = self.typed("PUT", &path, Some(to_value(&request))).await?;
        Ok(keys.into())
    }

    async fn health(&self) -> Result<LakeHealth, LakeError> {
        let health: wire::Health = self.typed("GET", &paths::health(), None).await?;
        Ok(health.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_error_code_is_read_as_the_protocol_says() {
        let codes = [
            ("invalid", 400, LakeError::Invalid("m".into())),
            ("refused", 403, LakeError::Refused("m".into())),
            ("unknown", 404, LakeError::Unknown("m".into())),
            ("conflict", 409, LakeError::Conflict("m".into())),
            ("lake", 502, LakeError::Lake("m".into())),
            ("unavailable", 503, LakeError::Unavailable("m".into())),
        ];
        for (code, status, expected) in codes {
            let retryable = matches!(code, "lake" | "unavailable");
            let body = format!(r#"{{"error":"m","code":"{code}","retryable":{retryable}}}"#);
            assert_eq!(answer("PUT x", status, &body), Err(expected));
        }
    }

    #[test]
    fn an_answer_not_of_the_protocol_is_the_lakes_fault() {
        assert!(matches!(
            answer("GET x", 500, "<html>"),
            Err(LakeError::Lake(_))
        ));
        assert!(matches!(
            answer("GET x", 503, ""),
            Err(LakeError::Unavailable(_))
        ));
        assert!(matches!(
            answer("GET x", 200, "nope"),
            Err(LakeError::Lake(_))
        ));
        assert_eq!(answer("PUT x", 204, ""), Ok(Value::Null));
    }
}
