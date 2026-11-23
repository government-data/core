//! The scheduler's rounds (decisions 0027, 0035): it runs on every
//! application VM, but only the one holding a PostgreSQL advisory lock
//! fires. The lock lives on a connection of its own, out of the pool, so
//! when that VM or its connection dies the lock goes with it and another
//! VM's next round takes it.

use chrono::{DateTime, Utc};
use sqlx::{Connection, PgConnection, PgPool};

use crate::firing::{self, Fired};

/// The advisory lock's key: `quality.scheduler`, as 64 bits.
const LOCK: i64 = 0x7164_7363_6865_6431;

/// One VM's scheduler.
#[derive(Debug)]
pub struct Scheduler {
    pool: PgPool,
    missed_max_hours: u32,
    /// The connection holding the lock, while this VM leads.
    leading: Option<PgConnection>,
}

impl Scheduler {
    /// `pool` is the application's; fires due longer ago than
    /// `missed_max_hours` are skipped.
    pub fn new(pool: PgPool, missed_max_hours: u32) -> Self {
        Self {
            pool,
            missed_max_hours,
            leading: None,
        }
    }

    /// Whether this VM leads: keeps the lock it holds, or tries to take it.
    pub async fn lead(&mut self) -> bool {
        if let Some(conn) = &mut self.leading {
            if conn.ping().await.is_ok() {
                return true;
            }
            tracing::warn!("scheduler: lost the lock's connection");
            self.leading = None;
        }
        let Ok(conn) = self.pool.acquire().await else {
            return false;
        };
        // Out of the pool: a pooled connection would be lent to others.
        let mut conn = conn.detach();
        match sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(LOCK)
            .fetch_one(&mut conn)
            .await
        {
            Ok(true) => {
                tracing::info!("scheduler: this VM fires the schedules");
                self.leading = Some(conn);
                true
            }
            _ => false,
        }
    }

    /// One round: if this VM leads, marks failed the cuts whose job failed
    /// and fires the due schedules; `None` if another VM leads. A failure is
    /// logged and fires nothing this round.
    pub async fn round(&mut self, now: DateTime<Utc>) -> Option<Vec<Fired>> {
        if !self.lead().await {
            return None;
        }
        let pool = &self.pool;
        let round = async {
            let failed = firing::fail_cuts(pool).await?;
            if failed > 0 {
                tracing::info!(failed, "scheduler: cuts whose job failed marked failed");
            }
            firing::fire_due(pool, now, self.missed_max_hours).await
        };
        match round.await {
            Ok(fired) => Some(fired),
            Err(e) => {
                tracing::error!("scheduler: {e}");
                Some(Vec::new())
            }
        }
    }
}
