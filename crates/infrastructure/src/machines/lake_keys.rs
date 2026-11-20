//! RustFS's root keys of each lake in Postgres (`lake_root_keys`, 0066 step
//! 11, the user's ruling of 2026-11-13): made here from the operating
//! system's CSPRNG the first time a lake's are asked for, the secret sealed
//! with the settings' key ([`SecretBox`]), and the same afterwards. Of two
//! first asks at once, the first row written wins and both answer it.

use async_trait::async_trait;
use auth_gateway::{Lake, LakeRootKeyStore, LakeRootKeys};
use domain::RepositoryError;
use ring::rand::{SecureRandom, SystemRandom};
use sqlx::postgres::PgPool;
use sqlx::Row;

use super::backend;
use crate::SecretBox;

/// A Postgres-backed [`LakeRootKeyStore`].
#[derive(Clone)]
pub struct PgLakeRootKeys {
    pool: PgPool,
    sealer: SecretBox,
    rng: SystemRandom,
}

impl std::fmt::Debug for PgLakeRootKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgLakeRootKeys").finish_non_exhaustive()
    }
}

impl PgLakeRootKeys {
    pub fn new(pool: PgPool, sealer: SecretBox) -> Self {
        Self {
            pool,
            sealer,
            rng: SystemRandom::new(),
        }
    }

    /// `bytes` random bytes, lowercase hex.
    fn random_hex(&self, bytes: usize) -> Result<String, RepositoryError> {
        let mut buf = vec![0u8; bytes];
        self.rng
            .fill(&mut buf)
            .map_err(|_| RepositoryError::Backend("no randomness".into()))?;
        Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
    }

    /// A new pair: an access key `qd` and 16 hex digits, a secret of 40 hex
    /// digits (160 bits), both what RustFS takes in an environment file.
    fn made(&self) -> Result<LakeRootKeys, RepositoryError> {
        Ok(LakeRootKeys {
            access_key: format!("qd{}", self.random_hex(8)?),
            secret_key: self.random_hex(20)?,
        })
    }
}

#[async_trait]
impl LakeRootKeyStore for PgLakeRootKeys {
    async fn keys(&self, lake: &Lake) -> Result<LakeRootKeys, RepositoryError> {
        let made = self.made()?;
        let sealed = self
            .sealer
            .encrypt(&made.secret_key)
            .map_err(RepositoryError::Backend)?;
        sqlx::query(
            "INSERT INTO lake_root_keys (lake, access_key, secret_key) VALUES ($1, $2, $3) \
             ON CONFLICT (tenant_id, lake) DO NOTHING",
        )
        .bind(lake.key())
        .bind(&made.access_key)
        .bind(&sealed)
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        let row = sqlx::query("SELECT access_key, secret_key FROM lake_root_keys WHERE lake = $1")
            .bind(lake.key())
            .fetch_one(&self.pool)
            .await
            .map_err(backend)?;
        let sealed: String = row.try_get("secret_key").map_err(backend)?;
        Ok(LakeRootKeys {
            access_key: row.try_get("access_key").map_err(backend)?,
            secret_key: self
                .sealer
                .decrypt(&sealed)
                .map_err(RepositoryError::Backend)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_pair_is_of_the_form_rustfs_takes_and_random() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused@127.0.0.1/unused")
            .unwrap();
        let store = PgLakeRootKeys::new(pool, SecretBox::from_key(&[7; 32]).unwrap());
        let (a, b) = (store.made().unwrap(), store.made().unwrap());
        assert_ne!(a, b);
        assert!(a.access_key.starts_with("qd") && a.access_key.len() == 18);
        assert_eq!(a.secret_key.len(), 40);
        let hex = |s: &str| s.chars().all(|c| c.is_ascii_hexdigit());
        assert!(hex(&a.access_key[2..]) && hex(&a.secret_key));
        assert_eq!(format!("{a:?}"), "LakeRootKeys(…)");
    }
}
