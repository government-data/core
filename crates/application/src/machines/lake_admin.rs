//! The lakes' platform admin keys in Settings → Lake (decision 0028, point
//! 2; 0066 step 11, the user's rulings of 2026-11-13): the replica's, which
//! `join.sh` makes on its node, and the main lake's (`pool:main`), which
//! `infra/rustfs/admin-user.sh` makes when its stack is brought up; each
//! shows its keys there once; a person who may
//! manage machines pastes them here, and they are kept sealed, replaced when
//! pasted again. The secret is never answered back; pasting is audited with
//! who and from where, never with the keys.

use std::sync::Arc;

use auth_gateway::{Lake, LakeAdminKeyStore, LakeAdminKeys, LakeAdminKeysKept};
use domain::AdminId;

use super::{audit, MachinesService, Origin};

/// Why a lake's admin keys were not kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LakeAdminError {
    /// Not 3 to 128 letters, digits, `.`, `_` or `-`.
    InvalidAccessKey,
    /// Not the 64 hex digits `join.sh` shows.
    InvalidSecret,
    /// The application has no store for them (no settings' key).
    Unavailable,
    Backend(String),
}

/// An access key RustFS takes: 3 to 128 letters, digits, `.`, `_` or `-`.
fn is_access_key(key: &str) -> bool {
    (3..=128).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A secret as `join.sh` makes it: 64 lowercase hex digits.
fn is_secret(secret: &str) -> bool {
    secret.len() == 64
        && secret
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl MachinesService {
    /// With the store of the lakes' admin keys.
    pub fn with_lake_admin_keys(mut self, keys: Arc<dyn LakeAdminKeyStore>) -> Self {
        self.lake_admin = Some(keys);
        self
    }

    fn lake_admin_store(&self) -> Result<&Arc<dyn LakeAdminKeyStore>, LakeAdminError> {
        self.lake_admin.as_ref().ok_or(LakeAdminError::Unavailable)
    }

    /// Keeps `lake`'s admin keys as a person pasted them (spaces around
    /// each dropped), in place of any kept; audited before it answers.
    pub async fn set_lake_admin_keys(
        &self,
        lake: &Lake,
        access_key: &str,
        secret_key: &str,
        by: &AdminId,
        origin: &Origin,
    ) -> Result<LakeAdminKeysKept, LakeAdminError> {
        let (access_key, secret_key) = (access_key.trim(), secret_key.trim());
        if !is_access_key(access_key) {
            return Err(LakeAdminError::InvalidAccessKey);
        }
        if !is_secret(secret_key) {
            return Err(LakeAdminError::InvalidSecret);
        }
        let store = self.lake_admin_store()?;
        let keys = LakeAdminKeys {
            access_key: access_key.to_string(),
            secret_key: secret_key.to_string(),
        };
        let now = self.clock.now();
        let backend = |e: domain::RepositoryError| LakeAdminError::Backend(e.to_string());
        let kept = store.set(lake, &keys, by, now).await.map_err(backend)?;
        let event = audit::lake_admin_keys_set(lake, by, origin, now);
        self.record(event).await.map_err(backend)?;
        Ok(kept)
    }

    /// What is kept of `lake`'s admin keys, without the secret.
    pub async fn lake_admin_keys(
        &self,
        lake: &Lake,
    ) -> Result<Option<LakeAdminKeysKept>, LakeAdminError> {
        let store = self.lake_admin_store()?;
        let kept = store.kept(lake).await;
        kept.map_err(|e| LakeAdminError::Backend(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_of_the_form_join_sh_shows() {
        assert!(is_access_key("qd-platform-admin"));
        for bad in ["qd", "with space", "quote\"", "ñandú", &"a".repeat(129)] {
            assert!(!is_access_key(bad), "{bad}");
        }
        assert!(is_secret(&"0a".repeat(32)));
        for bad in [
            "0a".repeat(31),
            "0A".repeat(32),
            "0g".repeat(32),
            "0a".repeat(33),
        ] {
            assert!(!is_secret(&bad), "{bad}");
        }
    }
}
