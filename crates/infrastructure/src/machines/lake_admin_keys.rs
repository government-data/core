//! The lakes' platform admin keys in Postgres (`lake_admin_keys`, 0066 step
//! 11, the user's ruling of 2026-11-13): as a person pasted them, the secret
//! sealed with the settings' key ([`SecretBox`]); pasted again, replaced.
//! The secret is opened only for the core's client of the lakes' adapters
//! ([`crate::lake_admins`]), through [`AdminKeySource`].

use std::time::SystemTime;

use async_trait::async_trait;
use auth_gateway::{Lake, LakeAdminKeyStore, LakeAdminKeys, LakeAdminKeysKept};
use chrono::{DateTime, Utc};
use domain::{AdminId, RepositoryError};
use sqlx::postgres::{PgPool, PgRow};
use sqlx::Row;

use super::backend;
use crate::lake_admin_client::AdminKeySource;
use crate::SecretBox;

/// A Postgres-backed [`LakeAdminKeyStore`].
#[derive(Clone)]
pub struct PgLakeAdminKeys {
    pool: PgPool,
    sealer: SecretBox,
}

impl std::fmt::Debug for PgLakeAdminKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgLakeAdminKeys").finish_non_exhaustive()
    }
}

impl PgLakeAdminKeys {
    pub fn new(pool: PgPool, sealer: SecretBox) -> Self {
        Self { pool, sealer }
    }
}

const KEPT: &str = "access_key, set_by::text AS set_by, set_at";

fn kept(row: &PgRow) -> Result<LakeAdminKeysKept, RepositoryError> {
    let at: DateTime<Utc> = row.try_get("set_at").map_err(backend)?;
    Ok(LakeAdminKeysKept {
        access_key: row.try_get("access_key").map_err(backend)?,
        set_by: AdminId::new(row.try_get::<String, _>("set_by").map_err(backend)?),
        set_at: SystemTime::from(at),
    })
}

#[async_trait]
impl LakeAdminKeyStore for PgLakeAdminKeys {
    async fn set(
        &self,
        lake: &Lake,
        keys: &LakeAdminKeys,
        by: &AdminId,
        at: SystemTime,
    ) -> Result<LakeAdminKeysKept, RepositoryError> {
        let sealed = self
            .sealer
            .encrypt(&keys.secret_key)
            .map_err(RepositoryError::Backend)?;
        let row = sqlx::query(&format!(
            "INSERT INTO lake_admin_keys (lake, access_key, secret_key, set_by, set_at) \
             VALUES ($1, $2, $3, $4::uuid, $5) \
             ON CONFLICT (tenant_id, lake) DO UPDATE SET access_key = EXCLUDED.access_key, \
             secret_key = EXCLUDED.secret_key, set_by = EXCLUDED.set_by, set_at = EXCLUDED.set_at \
             RETURNING {KEPT}"
        ))
        .bind(lake.key())
        .bind(&keys.access_key)
        .bind(&sealed)
        .bind(by.as_str())
        .bind(DateTime::<Utc>::from(at))
        .fetch_one(&self.pool)
        .await
        .map_err(backend)?;
        kept(&row)
    }

    async fn kept(&self, lake: &Lake) -> Result<Option<LakeAdminKeysKept>, RepositoryError> {
        let row = sqlx::query(&format!(
            "SELECT {KEPT} FROM lake_admin_keys WHERE lake = $1"
        ))
        .bind(lake.key())
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?;
        row.as_ref().map(kept).transpose()
    }
}

#[async_trait]
impl AdminKeySource for PgLakeAdminKeys {
    async fn open(&self, lake: &Lake) -> Result<Option<LakeAdminKeys>, RepositoryError> {
        let row = sqlx::query("SELECT access_key, secret_key FROM lake_admin_keys WHERE lake = $1")
            .bind(lake.key())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let sealed: String = row.try_get("secret_key").map_err(backend)?;
        let secret_key = self
            .sealer
            .decrypt(&sealed)
            .map_err(RepositoryError::Backend)?;
        Ok(Some(LakeAdminKeys {
            access_key: row.try_get("access_key").map_err(backend)?,
            secret_key,
        }))
    }
}
