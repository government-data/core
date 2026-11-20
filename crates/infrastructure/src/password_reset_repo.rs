//! Postgres adapter for the [`PasswordResetStore`] port, over the
//! `admin_password_resets` table.
//!
//! Timestamps cross the boundary as Unix epoch seconds, like the rest of the
//! auth adapters (see [`crate::admin_repo`]).

use std::time::SystemTime;

use async_trait::async_trait;
use auth_gateway::{PasswordHash, PasswordResetStore};
use domain::{AdminId, RepositoryError};
use sqlx::postgres::PgPool;
use sqlx::Row;

use crate::admin_repo::{backend, to_epoch};

/// A Postgres-backed [`PasswordResetStore`].
#[derive(Debug, Clone)]
pub struct PgPasswordResetStore {
    pool: PgPool,
}

impl PgPasswordResetStore {
    /// Build the store over an existing pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PasswordResetStore for PgPasswordResetStore {
    async fn issue(
        &self,
        admin: &AdminId,
        token_hash: &str,
        expires_at: SystemTime,
    ) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(backend)?;
        // Only the newest link works, and expired ones are not worth keeping.
        sqlx::query(
            "DELETE FROM admin_password_resets WHERE admin_id = $1::uuid OR expires_at <= now()",
        )
        .bind(admin.as_str())
        .execute(&mut *tx)
        .await
        .map_err(backend)?;
        sqlx::query(
            "INSERT INTO admin_password_resets (token_hash, admin_id, expires_at) \
             VALUES ($1, $2::uuid, to_timestamp($3))",
        )
        .bind(token_hash)
        .bind(admin.as_str())
        .bind(to_epoch(expires_at))
        .execute(&mut *tx)
        .await
        .map_err(backend)?;
        tx.commit().await.map_err(backend)
    }

    async fn find(
        &self,
        token_hash: &str,
        now: SystemTime,
    ) -> Result<Option<AdminId>, RepositoryError> {
        let row = sqlx::query(
            "SELECT admin_id::text AS admin_id FROM admin_password_resets \
             WHERE token_hash = $1 AND expires_at > to_timestamp($2)",
        )
        .bind(token_hash)
        .bind(to_epoch(now))
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?;
        row.map(|r| r.try_get::<String, _>("admin_id").map(AdminId::new))
            .transpose()
            .map_err(backend)
    }

    async fn redeem(
        &self,
        token_hash: &str,
        new_hash: &PasswordHash,
        now: SystemTime,
    ) -> Result<Option<AdminId>, RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(backend)?;
        // Deleting the row is what makes the link single-use: of two requests
        // racing with the same token, only one gets it back.
        let row = sqlx::query(
            "DELETE FROM admin_password_resets \
             WHERE token_hash = $1 AND expires_at > to_timestamp($2) \
             RETURNING admin_id::text AS admin_id",
        )
        .bind(token_hash)
        .bind(to_epoch(now))
        .fetch_optional(&mut *tx)
        .await
        .map_err(backend)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let admin: String = row.try_get("admin_id").map_err(backend)?;
        // A directory account's password lives in the directory; the link is
        // never issued for one, and this guard keeps it that way.
        let updated = sqlx::query(
            "UPDATE admin_users \
             SET password_hash = $2, failed_attempts = 0, locked_until = NULL, updated_at = now() \
             WHERE id = $1::uuid AND password_hash <> '!directory'",
        )
        .bind(&admin)
        .bind(new_hash.as_str())
        .execute(&mut *tx)
        .await
        .map_err(backend)?;
        if updated.rows_affected() == 0 {
            tx.commit().await.map_err(backend)?;
            return Ok(None);
        }
        for statement in [
            "DELETE FROM admin_password_resets WHERE admin_id = $1::uuid",
            "DELETE FROM admin_sessions WHERE admin_id = $1::uuid",
        ] {
            sqlx::query(statement)
                .bind(&admin)
                .execute(&mut *tx)
                .await
                .map_err(backend)?;
        }
        tx.commit().await.map_err(backend)?;
        Ok(Some(AdminId::new(admin)))
    }
}
