//! What a new machine is installed with (decision 0029, and 0066 step 11):
//! the join script, its role's compose file and the installation's CA, each
//! with its SHA-256, and the check that a join token may still be used,
//! which hands out the files that carry the installation's details without
//! using the token up.

use std::time::SystemTime;

use crate::domain::{MachineRole, RepositoryError};

/// A file a machine is installed with, and the SHA-256 of its bytes
/// (lowercase hex), except for the CA, whose SHA-256 is that of its DER:
/// the fingerprint `server ca-init` prints and the node agent checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallFile {
    pub body: String,
    pub sha256: String,
}

/// Port: the files of this version of the installation.
pub trait InstallFiles: Send + Sync {
    /// `join.sh`, which the join command downloads and checks.
    fn script(&self) -> InstallFile;

    /// The compose file a machine of `role` runs; `None` for a role that
    /// has none yet.
    fn compose(&self, role: MachineRole) -> Option<InstallFile>;

    /// Another file the compose file of `role` names (an entrypoint), by
    /// its name; `None` for one it does not name.
    fn file(&self, role: MachineRole, name: &str) -> Option<InstallFile> {
        let _ = (role, name);
        None
    }

    /// The installation's CA certificate, PEM; `None` without one.
    fn ca(&self) -> Option<InstallFile>;
}

/// What a usable join token is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinGrant {
    pub role: MachineRole,
    pub pool: Option<String>,
    /// The lake pool's node the token names, if it does.
    pub node: Option<String>,
}

/// Port: whether a join token may still be used, without using it.
#[async_trait::async_trait]
pub trait JoinTokenCheck: Send + Sync {
    /// What the token whose hash is `token_hash` is for, if it is unused and
    /// not expired at `now`. Nothing is changed.
    async fn usable(
        &self,
        token_hash: &str,
        now: SystemTime,
    ) -> Result<Option<JoinGrant>, RepositoryError>;
}

/// Which lake a pair of root keys is for: the replica, or a pool by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lake {
    Replica,
    Pool(String),
}

/// The pool that today's single RustFS (`infra/rustfs`) is, the
/// replication's source until a pool is installed (0066 step 11, the user's
/// ruling 1 of 2026-11-13): its admin keys are kept as `pool:main`.
pub const MAIN_POOL: &str = "main";

impl Lake {
    /// The main lake, [`MAIN_POOL`].
    pub fn main() -> Self {
        Self::Pool(MAIN_POOL.into())
    }

    /// From its stored key; `None` for one that is neither.
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "replica" => Some(Self::Replica),
            _ => key
                .strip_prefix("pool:")
                .filter(|n| !n.is_empty())
                .map(|n| Self::Pool(n.into())),
        }
    }

    /// As stored: `replica`, or `pool:<name>`.
    pub fn key(&self) -> String {
        match self {
            Self::Replica => "replica".into(),
            Self::Pool(name) => format!("pool:{name}"),
        }
    }
}

/// RustFS's root keys of a lake (0066 step 11, the user's ruling of
/// 2026-11-13): made by the application, the same for every node of the
/// lake, given to a node against its join token. Never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct LakeRootKeys {
    pub access_key: String,
    pub secret_key: String,
}

impl std::fmt::Debug for LakeRootKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LakeRootKeys(…)")
    }
}

/// Port: the lakes' root keys, made the first time they are asked for and
/// kept sealed; the same afterwards.
#[async_trait::async_trait]
pub trait LakeRootKeyStore: Send + Sync {
    async fn keys(&self, lake: &Lake) -> Result<LakeRootKeys, RepositoryError>;
}
