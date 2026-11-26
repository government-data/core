//! The module events in PostgreSQL (`module_events`, `module_event_cursors`,
//! quality-sdk decision 0017). A stored event notifies `module_events` with
//! its topic in the same transaction, so the notice goes out on commit.
//! `data` is never logged.

use application::events::{Consumer, EventsStore, NewEvent, StoredEvent};
use async_trait::async_trait;
use sqlx::postgres::PgPool;
use sqlx::Row;

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

    async fn after(
        &self,
        topic: &str,
        version_: u32,
        after: i64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, String> {
        let rows = sqlx::query(
            "SELECT seq, id::text AS id, publisher, occurred_at, data FROM module_events
             WHERE topic = $1 AND version = $2 AND seq > $3 ORDER BY seq LIMIT $4",
        )
        .bind(topic)
        .bind(version(version_)?)
        .bind(after)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                Ok(StoredEvent {
                    seq: r.try_get("seq").map_err(db)?,
                    id: r.try_get("id").map_err(db)?,
                    topic: topic.to_owned(),
                    version: version_,
                    publisher: r.try_get("publisher").map_err(db)?,
                    occurred_at: r.try_get("occurred_at").map_err(db)?,
                    data: r.try_get("data").map_err(db)?,
                })
            })
            .collect()
    }

    async fn cursor(&self, c: &Consumer) -> Result<Option<i64>, String> {
        sqlx::query_scalar(
            "SELECT cursor FROM module_event_cursors
             WHERE module = $1 AND topic = $2 AND version = $3",
        )
        .bind(&c.module)
        .bind(&c.topic)
        .bind(version(c.version)?)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)
    }

    async fn confirm(&self, c: &Consumer, cursor: i64) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO module_event_cursors (module, topic, version, cursor)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (module, topic, version) DO UPDATE
                 SET cursor = GREATEST(module_event_cursors.cursor, EXCLUDED.cursor),
                     confirmed_at = now()",
        )
        .bind(&c.module)
        .bind(&c.topic)
        .bind(version(c.version)?)
        .bind(cursor)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }
}
