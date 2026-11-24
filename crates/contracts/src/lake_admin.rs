//! The wire model of `lake-admin/1` (decision 0078): the provisioning
//! protocol the core speaks to a lake's adapter, HTTPS under
//! `/lake-admin/v1/` with JSON bodies. Pure types shared by the core's
//! client and the adapter; no I/O. No key, secret or policy shows in
//! `Debug`.

use serde::{Deserialize, Serialize};

use crate::LakeGrant;

/// The protocol's name and version, as `GET /about` lists it.
pub const PROTOCOL: &str = "lake-admin/1";

/// The base path every operation is under.
pub const BASE: &str = "/lake-admin/v1";

/// The operations' paths, under [`BASE`].
pub mod paths {
    use super::BASE;

    /// `GET`: [`super::About`].
    pub fn about() -> String {
        format!("{BASE}/about")
    }
    /// `GET`: [`super::Health`].
    pub fn health() -> String {
        format!("{BASE}/health")
    }
    /// `PUT` with [`super::RetentionRules`].
    pub fn warehouse(name: &str) -> String {
        format!("{BASE}/warehouses/{name}")
    }
    /// `PUT` with [`super::ReplicaTarget`], `GET`: [`super::ReplicationState`].
    pub fn replication(name: &str) -> String {
        format!("{BASE}/warehouses/{name}/replication")
    }
    /// `PUT` with [`super::ManagedPolicies`].
    pub fn managed_policies() -> String {
        format!("{BASE}/policies/managed")
    }
    /// `POST` with a [`crate::LakeGrant`]: [`super::SessionPolicy`].
    pub fn session_policy() -> String {
        format!("{BASE}/policies/session")
    }
    /// `PUT` with [`super::CredentialsRequest`]: [`super::Credentials`].
    pub fn credentials(kind: super::CredentialKind) -> String {
        format!("{BASE}/credentials/{}", kind.as_str())
    }
    /// `GET`: [`super::Pools`].
    pub fn pools() -> String {
        format!("{BASE}/pools")
    }
    /// `PUT` with [`super::PoolLayout`].
    pub fn pool(id: &str) -> String {
        format!("{BASE}/pools/{id}")
    }
}

/// `GET /about`: the protocol versions served and the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct About {
    pub protocols: Vec<String>,
    pub implementation: String,
    pub version: String,
}

/// `GET /health`: the lake's health, as far as it says it, asked with the
/// admin keys the core sent; or that it had none, or the lake refused them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Health {
    /// The lake answered: its drives, online and not.
    Answered {
        drives_online: u32,
        drives_offline: u32,
    },
    /// No admin keys came with the call: nothing was asked.
    NoKeys,
    /// The lake refused the keys: its own error code.
    KeysRefused { code: String },
}

/// `PUT /warehouses/{name}`: how long the warehouse keeps what is written.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionRules {
    #[serde(default)]
    pub versioning: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noncurrent_expire_days: Option<u32>,
}

/// `PUT /warehouses/{name}`'s answer: made now (`201`), there already, or
/// a step that failed (`200`), as the replication's answer says its state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WarehouseOutcome {
    Made,
    Present,
    Failed { step: String, detail: String },
}

/// `PUT /warehouses/{name}/replication`: the replica lake, as the lake
/// reaches it, and whether the source lake is a pool (the adapter works
/// out how its replication reaches the replica from it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplicaTarget {
    pub endpoint: String,
    #[serde(default)]
    pub source_pool: bool,
}

/// A warehouse's replication, as the lake reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ReplicationState {
    NotConfigured,
    Configured,
    Failed { step: String, detail: String },
}

/// What a managed access policy allows, on every warehouse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "access", content = "prefix", rename_all = "snake_case")]
pub enum ManagedAccess {
    ReadUnder(String),
    Read,
    ReadWrite,
    Administer,
}

/// One managed access policy, by the name it is signed in with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedPolicy {
    pub name: String,
    #[serde(flatten)]
    pub access: ManagedAccess,
}

/// `PUT /policies/managed`: the lake's managed policies, exactly these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedPolicies {
    pub policies: Vec<ManagedPolicy>,
}

/// `POST /policies/session`'s request.
pub type SessionPolicyRequest = LakeGrant;

/// `POST /policies/session`'s answer: the grant in the lake's dialect. Its
/// content never shows in `Debug`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPolicy {
    pub policy: String,
}

impl std::fmt::Debug for SessionPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SessionPolicy({} bytes)", self.policy.len())
    }
}

/// The lake's own credentials, by kind (0078's Open 4, ruled 2026-11-23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CredentialKind {
    Root,
    Admin,
    StsIssuer,
}

impl CredentialKind {
    /// As the path names it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Admin => "admin",
            Self::StsIssuer => "sts-issuer",
        }
    }
}

/// A pair of the lake's own keys. The secret never shows in `Debug`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    pub access_key: String,
    pub secret_key: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credentials(…)")
    }
}

/// `PUT /credentials/{kind}`: the keys the core keeps sealed, if any (the
/// adapter keeps none: 0078 point 9), and those the call is made with.
/// Its `Debug` shows no key.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<Credentials>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with: Option<Credentials>,
}

impl std::fmt::Debug for CredentialsRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialsRequest")
            .field("held", &self.held.is_some())
            .field("with", &self.with.is_some())
            .finish()
    }
}

/// A pool's layout: its nodes, each with its drives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolLayout {
    pub nodes: Vec<PoolNode>,
}

/// One node of a pool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolNode {
    pub name: String,
    pub drives: Vec<String>,
}

/// `GET /pools`: the lake's pools by id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pools {
    pub pools: Vec<Pool>,
}

/// One pool and its layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pool {
    pub id: String,
    #[serde(flatten)]
    pub layout: PoolLayout,
}

/// The error codes of a non-2xx answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Invalid,
    Refused,
    Unknown,
    Conflict,
    Lake,
    Unavailable,
}

impl ErrorCode {
    /// The HTTP status it is answered with.
    pub fn status(self) -> u16 {
        match self {
            Self::Invalid => 400,
            Self::Refused => 403,
            Self::Unknown => 404,
            Self::Conflict => 409,
            Self::Lake => 502,
            Self::Unavailable => 503,
        }
    }

    /// Whether the protocol says it may be tried again.
    pub fn retryable(self) -> bool {
        matches!(self, Self::Lake | Self::Unavailable)
    }
}

/// The body of a non-2xx answer. Its message names the operation and the
/// warehouse or pool, never a key or a policy's content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
    pub code: ErrorCode,
    pub retryable: bool,
}

impl ErrorBody {
    /// An error of `code`, retryable as the protocol says.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            error: message.into(),
            code,
            retryable: code.retryable(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::{json, Value};

    fn round_trip<T>(value: &T, wire: Value)
    where
        T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        assert_eq!(serde_json::to_value(value).unwrap(), wire);
        assert_eq!(&serde_json::from_value::<T>(wire).unwrap(), value);
    }

    #[test]
    fn every_path_is_under_the_base() {
        let all = [
            paths::about(),
            paths::health(),
            paths::warehouse("w"),
            paths::replication("w"),
            paths::managed_policies(),
            paths::session_policy(),
            paths::credentials(CredentialKind::StsIssuer),
            paths::pools(),
            paths::pool("p1"),
        ];
        assert!(all.iter().all(|p| p.starts_with("/lake-admin/v1/")));
        assert_eq!(
            paths::replication("w"),
            "/lake-admin/v1/warehouses/w/replication"
        );
        assert_eq!(
            paths::credentials(CredentialKind::StsIssuer),
            "/lake-admin/v1/credentials/sts-issuer"
        );
    }

    #[test]
    fn bodies_round_trip_as_the_protocol_writes_them() {
        round_trip(
            &About {
                protocols: vec![PROTOCOL.into()],
                implementation: "lake-rustfs".into(),
                version: "0.1.0".into(),
            },
            json!({"protocols": ["lake-admin/1"], "implementation": "lake-rustfs", "version": "0.1.0"}),
        );
        round_trip(
            &Health::Answered {
                drives_online: 4,
                drives_offline: 0,
            },
            json!({"state": "answered", "drives_online": 4, "drives_offline": 0}),
        );
        round_trip(&Health::NoKeys, json!({"state": "no_keys"}));
        round_trip(
            &Health::KeysRefused {
                code: "InvalidAccessKeyId".into(),
            },
            json!({"state": "keys_refused", "code": "InvalidAccessKeyId"}),
        );
        round_trip(&WarehouseOutcome::Made, json!({"state": "made"}));
        round_trip(
            &WarehouseOutcome::Failed {
                step: "warehouse_tables".into(),
                detail: "HTTP 403".into(),
            },
            json!({"state": "failed", "step": "warehouse_tables", "detail": "HTTP 403"}),
        );
        round_trip(
            &ReplicaTarget {
                endpoint: "lake-replica-1".into(),
                source_pool: true,
            },
            json!({"endpoint": "lake-replica-1", "source_pool": true}),
        );
        round_trip(
            &RetentionRules {
                versioning: true,
                locked_days: Some(30),
                noncurrent_expire_days: None,
            },
            json!({"versioning": true, "locked_days": 30}),
        );
        round_trip(
            &ReplicationState::Failed {
                step: "rule".into(),
                detail: "x".into(),
            },
            json!({"state": "failed", "step": "rule", "detail": "x"}),
        );
        round_trip(
            &ReplicationState::NotConfigured,
            json!({"state": "not_configured"}),
        );
        round_trip(
            &ManagedPolicies {
                policies: vec![
                    ManagedPolicy {
                        name: "quality-viewer".into(),
                        access: ManagedAccess::ReadUnder("reports/".into()),
                    },
                    ManagedPolicy {
                        name: "quality-admin".into(),
                        access: ManagedAccess::Administer,
                    },
                ],
            },
            json!({"policies": [
                {"name": "quality-viewer", "access": "read_under", "prefix": "reports/"},
                {"name": "quality-admin", "access": "administer"}]}),
        );
        round_trip(
            &Pools {
                pools: vec![Pool {
                    id: "p1".into(),
                    layout: PoolLayout {
                        nodes: vec![PoolNode {
                            name: "lake-1".into(),
                            drives: vec!["/srv/d1".into()],
                        }],
                    },
                }],
            },
            json!({"pools": [{"id": "p1", "nodes": [{"name": "lake-1", "drives": ["/srv/d1"]}]}]}),
        );
        round_trip(
            &ErrorBody::new(ErrorCode::Lake, "PUT warehouse w"),
            json!({"error": "PUT warehouse w", "code": "lake", "retryable": true}),
        );
    }

    #[test]
    fn errors_answer_with_the_protocols_status() {
        let codes = [
            (ErrorCode::Invalid, 400, false),
            (ErrorCode::Refused, 403, false),
            (ErrorCode::Unknown, 404, false),
            (ErrorCode::Conflict, 409, false),
            (ErrorCode::Lake, 502, true),
            (ErrorCode::Unavailable, 503, true),
        ];
        for (code, status, retryable) in codes {
            assert_eq!(
                (code.status(), code.retryable()),
                (status, retryable),
                "{code:?}"
            );
        }
    }

    #[test]
    fn no_key_secret_or_policy_shows_in_debug() {
        let keys = Credentials {
            access_key: "the-access".into(),
            secret_key: "the-secret".into(),
        };
        let request = CredentialsRequest {
            held: Some(keys.clone()),
            with: Some(keys.clone()),
        };
        let policy = SessionPolicy {
            policy: "{\"Statement\":[]}".into(),
        };
        let shown = format!("{keys:?} {request:?} {policy:?}");
        for hidden in ["the-access", "the-secret", "Statement"] {
            assert!(!shown.contains(hidden), "{shown}");
        }
        let wire = serde_json::to_value(&request).unwrap();
        assert_eq!(
            wire["held"]["secret_key"], "the-secret",
            "on the wire, inside mTLS"
        );
    }
}
