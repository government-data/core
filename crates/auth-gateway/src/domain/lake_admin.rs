//! The platform's admin user of a lake (decision 0028, point 2; 0066 step
//! 11, the user's ruling of 2026-11-13): `join.sh` makes it on the node when
//! it brings the lake up and shows its keys there once; a person pastes them
//! in Settings → Lake, and the application keeps them sealed. They are never
//! answered back, logged or audited. What a lake answers with them is
//! `application::lake`'s (decision 0078, step 10).

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
