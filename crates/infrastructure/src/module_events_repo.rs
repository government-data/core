//! The module events in PostgreSQL (`module_events`, quality-sdk decision
//! 0017). A stored event notifies `module_events` with
//! its topic in the same transaction, so the notice goes out on commit.
//! `data` is never logged.

use application::events::{EventsStore, NewEvent};
use async_trait::async_trait;
use sqlx::postgres::PgPool;

/// The channel a stored event is notified on.
pub const CHANNEL: &str = "module_events";

/// [`EventsStore`] on PostgreSQL.
#[derive(Debug, Clone)]
pub struct PgModuleEvents {
    pool: PgPool,
}

impl PgModuleEvents {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn db(e: sqlx::Error) -> String {
    format!("database: {e}")
}

fn version(v: u32) -> Result<i32, String> {
    i32::try_from(v).map_err(|_| format!("version {v} is out of range"))
}

#[async_trait]
impl EventsStore for PgModuleEvents {
    async fn store(&self, publisher: &str, event: &NewEvent) -> Result<bool, String> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let stored: Option<i64> = sqlx::query_scalar(
            "INSERT INTO module_events (id, topic, version, publisher, occurred_at, data)
             VALUES ($1::uuid, $2, $3, $4, $5, $6)
             ON CONFLICT (id) DO NOTHING RETURNING seq",
        )
        .bind(&event.id)
        .bind(&event.topic)
        .bind(version(event.version)?)
        .bind(publisher)
        .bind(event.occurred_at)
        .bind(&event.data)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        if stored.is_some() {
            sqlx::query("SELECT pg_notify($1, $2)")
                .bind(CHANNEL)
                .bind(&event.topic)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        tx.commit().await.map_err(db)?;
        Ok(stored.is_some())
    }
}
