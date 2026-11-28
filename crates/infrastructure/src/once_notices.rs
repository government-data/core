//! The once-notices that went out (`once_notices`, decision 0090, ruling
//! 78), on Postgres.

use application::once_notices::OnceNotices;
use async_trait::async_trait;
use sqlx::PgPool;

#[derive(Debug)]
pub struct PgOnceNotices {
    pool: PgPool,
}

impl PgOnceNotices {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl OnceNotices for PgOnceNotices {
    async fn sent(&self, key: &str) -> Result<bool, String> {
        let found: Option<i32> = sqlx::query_scalar("SELECT 1 FROM once_notices WHERE key = $1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(found.is_some())
    }

    async fn mark_sent(&self, key: &str) -> Result<bool, String> {
        let done = sqlx::query("INSERT INTO once_notices (key) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(key)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(done.rows_affected() == 1)
    }
}
