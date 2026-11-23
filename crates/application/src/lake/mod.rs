//! The lake as the capability `lake` (decision 0078): two ports that name
//! no vendor. [`Lake`] is the data, spoken over the standard protocols (S3,
//! Iceberg REST, STS); [`LakeAdmin`] is the provisioning, whose one
//! implementation in the core is the client of the lake's adapter
//! (`lake-admin/1`). What a job may do is a [`LakeGrant`], never a policy.
//! Nothing is wired to them yet.

mod properties;
mod wire;

use std::time::{Duration, SystemTime};

use async_trait::async_trait;
pub use contracts::{LakeGrant, LakeRead, LakeTable};

pub use properties::{CatalogPrefix, LakeProperties, UrlStyle};

/// Why a lake did not do what it was asked, as `lake-admin/1`'s error
/// codes say it. The message names the operation and the warehouse,
/// never a key or a policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LakeError {
    /// The request itself (a grant that does not check, a bad name).
    Invalid(String),
    /// The caller's certificate was refused.
    Refused(String),
    /// No such warehouse or pool.
    Unknown(String),
    /// The lake holds a state the request cannot reach.
    Conflict(String),
    /// The lake did not answer as expected; worth trying again.
    Lake(String),
    /// Not reachable now; worth trying again.
    Unavailable(String),
}

impl LakeError {
    /// Whether trying again later may succeed.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Lake(_) | Self::Unavailable(_))
    }
}

impl std::fmt::Display for LakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (code, message) = match self {
            Self::Invalid(m) => ("invalid", m),
            Self::Refused(m) => ("refused", m),
            Self::Unknown(m) => ("unknown", m),
            Self::Conflict(m) => ("conflict", m),
            Self::Lake(m) => ("lake", m),
            Self::Unavailable(m) => ("unavailable", m),
        };
        write!(f, "{code}: {message}")
    }
}

impl std::error::Error for LakeError {}

/// A job's temporary credentials, issued by STS within a grant's session
/// policy. The secret and the token never show in `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct LakeCredentials {
    pub access_key: String,
    pub secret_key: String,
    pub session_token: String,
    pub expires_at: SystemTime,
}

impl std::fmt::Debug for LakeCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LakeCredentials")
            .field("access_key", &self.access_key)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// Whether an object written once was written now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOnce {
    Written,
    /// It was there already (`If-None-Match: *` refused): left as it was.
    AlreadyThere,
}

/// Port: the lake's data, over its standard protocols.
#[async_trait]
pub trait Lake: Send + Sync {
    /// What the lake declares it supports.
    fn properties(&self) -> &LakeProperties;
    /// A job's credentials within `grant`, for at least `duration`;
    /// `session` names them in the lake's logs.
    async fn credentials(
        &self,
        grant: &LakeGrant,
        session: &str,
        duration: Duration,
    ) -> Result<LakeCredentials, LakeError>;
    /// Makes `namespace` unless it is there.
    async fn ensure_namespace(&self, warehouse: &str, namespace: &str) -> Result<(), LakeError>;
    /// The tables of `namespace`, by name; none if it is not there.
    async fn tables(&self, warehouse: &str, namespace: &str) -> Result<Vec<String>, LakeError>;
    /// The table's prefix in the warehouse, as the catalog says; `None` if
    /// there is no such table.
    async fn table_location(
        &self,
        warehouse: &str,
        namespace: &str,
        table: &str,
    ) -> Result<Option<String>, LakeError>;
    /// Drops the table with its files, and its namespace if left empty;
    /// `false` if there was no such table.
    async fn drop_table(
        &self,
        warehouse: &str,
        namespace: &str,
        table: &str,
    ) -> Result<bool, LakeError>;
    /// Writes `body` at `key` unless an object is there.
    async fn write_once(
        &self,
        warehouse: &str,
        key: &str,
        body: &[u8],
    ) -> Result<WriteOnce, LakeError>;
}

/// How long a warehouse keeps what is written (0028's retention).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetentionRules {
    /// Every write kept as a version.
    pub versioning: bool,
    /// Days a version cannot be deleted or shortened (object lock, in
    /// compliance mode); `None` for no lock.
    pub locked_days: Option<u32>,
    /// Days after which a noncurrent version expires; `None` never.
    pub noncurrent_expire_days: Option<u32>,
}

/// Where a warehouse is replicated: the replica lake, as the lake reaches
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaTarget {
    pub endpoint: String,
}

/// A warehouse's replication, as the lake reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplicationState {
    NotConfigured,
    Configured,
    /// Configured in part: the step it stopped at, in a few words.
    Failed {
        step: String,
        detail: String,
    },
}

/// What a managed access policy (0028) allows, on every warehouse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedAccess {
    /// List the warehouses and their objects, and read under a prefix.
    ReadUnder(String),
    /// Read every object.
    Read,
    /// Read and write every object.
    ReadWrite,
    /// Everything, the lake's administration included.
    Administer,
}

/// A managed access policy, by the name a person or role signs in with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPolicy {
    pub name: String,
    pub access: ManagedAccess,
}

/// A grant written in the lake's own policy language. Its content never
/// shows in `Debug` or in a log.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionPolicy(pub String);

impl std::fmt::Debug for SessionPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SessionPolicy({} bytes)", self.0.len())
    }
}

/// The lake's own credentials (the user's ruling of 2026-11-23 on 0078's
/// Open 4); a job's are [`Lake::credentials`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LakeCredentialKind {
    Root,
    Admin,
    StsIssuer,
}

/// A pair of the lake's own keys, kept sealed by the core (point 9). The
/// secret never shows in `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct LakeKeys {
    pub access_key: String,
    pub secret_key: String,
}

impl std::fmt::Debug for LakeKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LakeKeys(…)")
    }
}

/// The lake's health, as far as it says it (0078's Open 7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LakeHealth {
    pub healthy: bool,
    pub drives_online: Option<u32>,
    pub drives_offline: Option<u32>,
}

/// Port: the lake's provisioning. Every operation can run twice: run again
/// with the same input, it changes nothing.
#[async_trait]
pub trait LakeAdmin: Send + Sync {
    /// Makes the warehouse unless it is there, with `rules`.
    async fn ensure_warehouse(&self, name: &str, rules: &RetentionRules) -> Result<(), LakeError>;
    /// Replicates the warehouse into `replica`, made there unless it is.
    async fn replicate(
        &self,
        name: &str,
        replica: &ReplicaTarget,
    ) -> Result<ReplicationState, LakeError>;
    /// The warehouse's replication as it stands.
    async fn replication(&self, name: &str) -> Result<ReplicationState, LakeError>;
    /// Sets the lake's managed policies to exactly `policies`.
    async fn set_managed_policies(&self, policies: &[ManagedPolicy]) -> Result<(), LakeError>;
    /// `grant` in the lake's policy language; changes nothing.
    async fn session_policy(&self, grant: &LakeGrant) -> Result<SessionPolicy, LakeError>;
    /// The lake's own keys of `kind`: `held` (the ones the core keeps) if
    /// the lake still takes them, else new ones made, for the core to seal.
    async fn ensure_credentials(
        &self,
        kind: LakeCredentialKind,
        held: Option<&LakeKeys>,
    ) -> Result<LakeKeys, LakeError>;
    /// The lake's health now.
    async fn health(&self) -> Result<LakeHealth, LakeError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_secret_shows_in_debug() {
        let credentials = LakeCredentials {
            access_key: "AK".into(),
            secret_key: "the-secret".into(),
            session_token: "the-token".into(),
            expires_at: SystemTime::UNIX_EPOCH,
        };
        let keys = LakeKeys {
            access_key: "AK".into(),
            secret_key: "the-secret".into(),
        };
        let policy = SessionPolicy("{\"Statement\":[]}".into());
        let shown = format!("{credentials:?} {keys:?} {policy:?}");
        for secret in ["the-secret", "the-token", "Statement"] {
            assert!(!shown.contains(secret), "{shown}");
        }
        assert!(shown.contains("AK"));
    }

    #[test]
    fn only_the_lakes_own_trouble_is_worth_trying_again() {
        assert!(LakeError::Lake("x".into()).retryable());
        assert!(LakeError::Unavailable("x".into()).retryable());
        for e in [
            LakeError::Invalid("x".into()),
            LakeError::Refused("x".into()),
            LakeError::Unknown("x".into()),
            LakeError::Conflict("x".into()),
        ] {
            assert!(!e.retryable(), "{e}");
        }
        assert_eq!(
            LakeError::Unknown("warehouse w".into()).to_string(),
            "unknown: warehouse w"
        );
    }
}
