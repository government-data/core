//! Whether a join token may still be used, without using it (0066 step 11):
//! the role's compose file is given to whoever presents one.

use std::time::SystemTime;

use async_trait::async_trait;
use auth_gateway::{JoinGrant, JoinTokenCheck, MachineRole};
use chrono::{DateTime, Utc};
use domain::RepositoryError;
use sqlx::Row;

use super::{backend, PgMachines};

#[async_trait]
impl JoinTokenCheck for PgMachines {
    async fn usable(
        &self,
        token_hash: &str,
        now: SystemTime,
    ) -> Result<Option<JoinGrant>, RepositoryError> {
        let row = sqlx::query(
            "SELECT role, pool, node FROM join_tokens \
             WHERE token_hash = $1 AND used_at IS NULL AND expires_at > $2",
        )
        .bind(token_hash)
        .bind(DateTime::<Utc>::from(now))
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let role: String = row.try_get("role").map_err(backend)?;
        Ok(Some(JoinGrant {
            role: MachineRole::parse(&role)
                .ok_or_else(|| RepositoryError::Backend(format!("role {role:?}")))?,
            pool: row.try_get("pool").map_err(backend)?,
            node: row.try_get("node").map_err(backend)?,
        }))
    }
}
