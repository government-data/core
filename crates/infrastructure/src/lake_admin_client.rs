//! The core's client of a lake's adapter over `lake-admin/1` (decision
//! 0078, step 6): `application::lake::LakeAdmin` at the wire, HTTPS with
//! the core server's machine certificate, trusting the installation's CA
//! alone and accepting only an adapter's certificate (the role of its
//! lake's machines: `lake`, or `lake-replica` for the replica). A call
//! is tried again only when the adapter says it may be (`retryable`) or
//! did not answer. No body is logged: neither a key, a policy nor an
//! answer of `/credentials`.
//!
//! The keys an operation needs (step 11, the user's ruling of 2026-11-24)
//! are read from the core's sealed store for each call and sent in its
//! headers (`contracts::lake_admin::headers`), never logged.

use std::sync::Arc;
use std::time::Duration;

use application::lake::{
    LakeAdmin, LakeCredentialKind, LakeError, LakeGrant, LakeHealth, LakeKeys, ManagedPolicy,
    ReplicaTarget, ReplicationState, ReplicationSync, RetentionRules, SessionPolicy, WarehouseMade,
};
use async_trait::async_trait;
use auth_gateway::{Lake, LakeAdminKeys};
use contracts::lake_admin::{self as wire, headers, paths};
use domain::RepositoryError;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// The role an adapter's certificate carries, unless said otherwise.
pub const ADAPTER_ROLE: &str = "lake";

/// How many times a call is made at most.
const ATTEMPTS: u32 = 3;

/// Where the client reads a lake's admin keys, the secret opened. Only the
/// clients of the lakes' adapters are given it.
#[async_trait]
pub trait AdminKeySource: Send + Sync {
    async fn open(&self, lake: &Lake) -> Result<Option<LakeAdminKeys>, RepositoryError>;
}

/// The headers of one call: a name and its value.
type Headers = Vec<(&'static str, String)>;

/// A client of one adapter.
#[derive(Clone)]
pub struct LakeAdminClient {
    base: String,
    http: reqwest::Client,
    pause: Duration,
    role: &'static str,
    keys: Option<(Arc<dyn AdminKeySource>, Lake)>,
}

impl std::fmt::Debug for LakeAdminClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LakeAdminClient")
            .field("base", &self.base)
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
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
            role: ADAPTER_ROLE,
            keys: None,
        })
    }

    /// The same client, accepting an adapter of `role` instead.
    pub fn accepting(mut self, role: &'static str) -> Self {
        self.role = role;
        self
    }

    /// The same client, sending `lake`'s keys (and the replica's, to
    /// replicate) from `source`.
    pub fn with_keys(mut self, source: Arc<dyn AdminKeySource>, lake: Lake) -> Self {
        self.keys = Some((source, lake));
        self
    }

    /// `GET /about`: the protocol versions the adapter serves, and what it
    /// is.
    pub async fn about(&self) -> Result<wire::About, LakeError> {
        self.typed("GET", &paths::about(), None, &Vec::new()).await
    }

    /// `lake`'s keys in `header`, if they are kept.
    async fn header(&self, lake: &Lake, header: &'static str) -> Result<Headers, LakeError> {
        let Some((source, _)) = &self.keys else {
            return Ok(Vec::new());
        };
        let kept = source.open(lake).await;
        let kept = kept.map_err(|e| LakeError::Unavailable(format!("the kept keys: {e}")))?;
        Ok(kept
            .map(|k| {
                let keys = wire::Credentials {
                    access_key: k.access_key,
                    secret_key: k.secret_key,
                };
                (header, headers::encode(&keys))
            })
            .into_iter()
            .collect())
    }

    /// The lake's own admin keys, as headers.
    async fn admin_keys(&self) -> Result<Headers, LakeError> {
        match &self.keys {
            Some((_, lake)) => self.header(lake, headers::ADMIN_KEYS).await,
            None => Ok(Vec::new()),
        }
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
        headers: &Headers,
    ) -> Result<Value, LakeError> {
        let mut attempt = 1;
        loop {
            match self.once(method, path, body.as_ref(), headers).await {
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
        headers: &Headers,
    ) -> Result<Value, LakeError> {
        let what = format!("{method} {path}");
        let verb = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|e| LakeError::Invalid(format!("{what}: {e}")))?;
        let mut request = self.http.request(verb, format!("{}{path}", self.base));
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|e| LakeError::Unavailable(format!("{what}: {}", short(&e))))?;
        if !is_role(&response, self.role) {
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
        headers: &Headers,
    ) -> Result<T, LakeError> {
        let value = self.call(method, path, body, headers).await?;
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

/// Whether the peer presented a certificate of `role`.
fn is_role(response: &reqwest::Response, role: &str) -> bool {
    let Some(info) = response.extensions().get::<reqwest::tls::TlsInfo>() else {
        return false;
    };
    let Some(der) = info.peer_certificate() else {
        return false;
    };
    let Ok((_, cert)) = x509_parser::parse_x509_certificate(der) else {
        return false;
    };
    let ou = cert.subject().iter_organizational_unit().next();
    let matches = ou.and_then(|r| r.as_str().ok()) == Some(role);
    matches
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
    async fn ensure_warehouse(
        &self,
        name: &str,
        rules: &RetentionRules,
    ) -> Result<WarehouseMade, LakeError> {
        let body = to_value(&wire::RetentionRules::from(rules));
        let keys = self.admin_keys().await?;
        let made: wire::WarehouseOutcome = self
            .typed("PUT", &paths::warehouse(name), Some(body), &keys)
            .await?;
        Ok(made.into())
    }

    async fn replicate(
        &self,
        name: &str,
        replica: &ReplicaTarget,
    ) -> Result<ReplicationState, LakeError> {
        let body = to_value(&wire::ReplicaTarget::from(replica));
        let mut keys = self.admin_keys().await?;
        keys.extend(self.header(&Lake::Replica, headers::REPLICA_KEYS).await?);
        let state: wire::ReplicationState = self
            .typed("PUT", &paths::replication(name), Some(body), &keys)
            .await?;
        Ok(state.into())
    }

    async fn replication(&self, name: &str) -> Result<ReplicationSync, LakeError> {
        let keys = self.admin_keys().await?;
        let sync: wire::ReplicationSync = self
            .typed("GET", &paths::replication(name), None, &keys)
            .await?;
        Ok(sync.into())
    }

    async fn set_managed_policies(&self, policies: &[ManagedPolicy]) -> Result<(), LakeError> {
        let body = to_value(&wire::ManagedPolicies {
            policies: policies.iter().map(Into::into).collect(),
        });
        self.call("PUT", &paths::managed_policies(), Some(body), &Vec::new())
            .await
            .map(|_| ())
    }

    async fn session_policy(&self, grant: &LakeGrant) -> Result<SessionPolicy, LakeError> {
        let policy: wire::SessionPolicy = self
            .typed(
                "POST",
                &paths::session_policy(),
                Some(to_value(grant)),
                &Vec::new(),
            )
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
        let body = Some(to_value(&request));
        let keys: wire::Credentials = self.typed("PUT", &path, body, &Vec::new()).await?;
        Ok(keys.into())
    }

    async fn health(&self) -> Result<LakeHealth, LakeError> {
        let keys = self.admin_keys().await?;
        let health: wire::Health = self.typed("GET", &paths::health(), None, &keys).await?;
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
