//! Signing in with the email (online edition): the one-time passes to each
//! subdomain (`login_handoffs`, of the current company). The companies an
//! email is in come from [`crate::tenants_repo::PgTenantDirectory`].

use std::time::SystemTime;

use async_trait::async_trait;
use auth_gateway::LoginHandoffStore;
use domain::{AdminId, RepositoryError};
use sqlx::{PgPool, Row};

use crate::admin_repo::{backend, to_epoch};

#[derive(Debug, Clone)]
pub struct PgLoginHandoffs {
    pool: PgPool,
}

impl PgLoginHandoffs {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl LoginHandoffStore for PgLoginHandoffs {
    async fn issue(
        &self,
        admin: &AdminId,
        token_hash: &str,
        expires_at: SystemTime,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO login_handoffs (token_hash, admin_id, expires_at) \
             VALUES ($1, $2::uuid, to_timestamp($3))",
        )
        .bind(token_hash)
        .bind(admin.as_str())
        .bind(to_epoch(expires_at))
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        // The company's expired ones are no use.
        sqlx::query("DELETE FROM login_handoffs WHERE expires_at <= now()")
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn redeem(
        &self,
        token_hash: &str,
        now: SystemTime,
    ) -> Result<Option<AdminId>, RepositoryError> {
        // Deleting it is what makes it single-use: of two requests with the
        // same pass, only one gets it.
        let row = sqlx::query(
            "DELETE FROM login_handoffs \
             WHERE token_hash = $1 AND expires_at > to_timestamp($2) \
             RETURNING admin_id::text AS admin_id",
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
}
