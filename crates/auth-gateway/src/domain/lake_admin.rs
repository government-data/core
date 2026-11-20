//! The platform's admin user of a lake (decision 0028, point 2; 0066 step
//! 11, the user's ruling of 2026-11-13): `join.sh` makes it on the node when
//! it brings the lake up and shows its keys there once; a person pastes them
//! in Settings → Lake, and the application keeps them sealed. They are never
//! answered back, logged or audited.

use std::time::SystemTime;

use crate::domain::{AdminId, Lake, RepositoryError};

/// A lake's admin user's keys, as a person pasted them. Never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct LakeAdminKeys {
    pub access_key: String,
    pub secret_key: String,
}

impl std::fmt::Debug for LakeAdminKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LakeAdminKeys(…)")
    }
}

/// What may be shown of a lake's admin keys once kept: the access key, who
/// pasted them and when; never the secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LakeAdminKeysKept {
    pub access_key: String,
    pub set_by: AdminId,
    pub set_at: SystemTime,
}

/// Port: the lakes' admin keys, the secret sealed.
#[async_trait::async_trait]
pub trait LakeAdminKeyStore: Send + Sync {
    /// Keeps `keys` for `lake`, in place of any kept before.
    async fn set(
        &self,
        lake: &Lake,
        keys: &LakeAdminKeys,
        by: &AdminId,
        at: SystemTime,
    ) -> Result<LakeAdminKeysKept, RepositoryError>;

    /// What is kept for `lake`, without the secret; `None` before a person
    /// pasted them.
    async fn kept(&self, lake: &Lake) -> Result<Option<LakeAdminKeysKept>, RepositoryError>;
}

/// What a lake answered when the application asked it, with the admin keys
/// kept for it, for its admin API's `info` (decision 0028, "Lake health";
/// 0066 step 11, verified on 2026-11-13 with RustFS 1.0.5). Holds no key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LakeProbe {
    /// No admin keys are kept for the lake: nothing was asked.
    NoKeys,
    /// The keys were taken; its drives, online and not.
    Answered {
        drives_online: u32,
        drives_offline: u32,
    },
    /// RustFS refused the keys (`SignatureDoesNotMatch`,
    /// `InvalidAccessKeyId`, `AccessDenied`): its error's code.
    KeysRefused { code: String },
    /// It could not be reached, or answered something else: what happened,
    /// in a few words, without a key.
    Unreachable { detail: String },
}

/// Port: a lake's admin API (0028's `LakeAdmin`), signed with the admin
/// keys kept for the lake, which only its adapter reads.
#[async_trait::async_trait]
pub trait LakeAdmin: Send + Sync {
    /// Asks `node` of `lake` for its `info`: a node's name, reached on its
    /// gate's port, or the lake's base URL (the main lake's).
    async fn probe(&self, lake: &Lake, node: &str) -> Result<LakeProbe, RepositoryError>;
}
