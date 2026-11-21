//! The quality work an update would cut (decision 0003): jobs a worker
//! holds, jobs waiting in the queue, and schedules due soon. Counted on the
//! API's connection, which sees the installation's own rows.

use std::time::Duration;

use contracts::{Drain, Work};
use sqlx::{PgPool, Row};

/// The quality module's [`Drain`], over its queue and its schedules.
#[derive(Debug, Clone)]
pub struct QualityDrain {
    pool: PgPool,
}

impl QualityDrain {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl Drain for QualityDrain {
    async fn work(&self, horizon: Duration) -> Result<Work, String> {
        let row = sqlx::query(
            "SELECT
                 (SELECT count(*) FROM quality.jobs WHERE state = 'running') AS running,
                 (SELECT count(*) FROM quality.jobs WHERE state = 'queued') AS queued,
                 (SELECT count(*) FROM quality.schedules
                   WHERE enabled AND next_fire_at <= now() + make_interval(secs => $1))
                   AS scheduled_soon",
        )
        .bind(horizon.as_secs_f64())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| format!("quality work: {e}"))?;
        let n = |c: &str| -> Result<u64, String> {
            let v: i64 = row.try_get(c).map_err(|e| format!("quality work: {e}"))?;
            Ok(u64::try_from(v).unwrap_or_default())
        };
        Ok(Work {
            running: n("running")?,
            queued: n("queued")?,
            scheduled_soon: n("scheduled_soon")?,
        })
    }
}
