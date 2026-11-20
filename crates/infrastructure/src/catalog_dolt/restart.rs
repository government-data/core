//! Dolt restarted when free, checked twice (decision 0066, points 9 and
//! 11): the API is Dolt's only client, so the first fact is the API's own
//! gate (no operation of the catalog in flight; the next ones held), and
//! the second is Dolt's: its `information_schema.processlist` shows no
//! other connection running a statement. Before asking Dolt, every pool is
//! closed, so no connection of the API is left, and with it no open
//! transaction, which Dolt's list cannot see (it shows as `Sleep`,
//! `tools/verify/dolt_idle_restart.sh`). Pools are made again on the next
//! operation, against the restarted Dolt.

use std::time::Duration;

use sqlx::{Connection, MySqlConnection};

use super::DoltCatalog;

/// How long an operation waits for a hold to end: longer than Dolt's
/// restart (about 3 seconds after `docker restart`) and the agent's checks.
pub const HOLD_WAIT: Duration = Duration::from_secs(30);

/// The answer to Dolt's agent asking whether it may restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoltHold {
    /// Nothing in flight here and nothing running in Dolt: held until
    /// released or expired. Otherwise nothing is held.
    pub free: bool,
    /// The catalog's operations in flight in this API.
    pub in_flight: u64,
    /// The other connections running a statement in Dolt; `None` when Dolt
    /// did not answer, which is not free.
    pub running: Option<u64>,
}

impl DoltCatalog {
    /// Holds the catalog for `ttl` if Dolt is free: first this API's
    /// operations (none in flight, the next ones wait), its pools closed,
    /// then Dolt asked. Not free, nothing stays held.
    pub async fn hold(&self, ttl: Duration) -> DoltHold {
        if let Err(in_flight) = self.gate.hold(ttl) {
            return DoltHold {
                free: false,
                in_flight: u64::from(in_flight),
                running: None,
            };
        }
        self.close_pools().await;
        let running = match self.running_statements().await {
            Ok(n) => Some(n),
            Err(e) => {
                tracing::warn!("Dolt did not say whether it runs statements: {e}");
                None
            }
        };
        let free = running == Some(0);
        if !free {
            self.gate.release();
        }
        DoltHold {
            free,
            in_flight: 0,
            running,
        }
    }

    /// Ends the hold: whether one was in force.
    pub fn release(&self) -> bool {
        self.gate.release()
    }

    /// Closes every pool; the next operation makes its pool again.
    async fn close_pools(&self) {
        let pools: Vec<_> = {
            let mut pools = self.pools.lock().expect("catalog pools");
            pools.drain().map(|(_, pool)| pool).collect()
        };
        for pool in pools {
            pool.close().await;
        }
    }

    /// The other connections in `Query`, by Dolt's own list, asked on a
    /// connection of its own (without a database) that is closed after.
    async fn running_statements(&self) -> Result<u64, sqlx::Error> {
        let mut conn = MySqlConnection::connect(&self.base).await?;
        let running: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM information_schema.processlist \
             WHERE command = 'Query' AND id <> CONNECTION_ID()",
        )
        .fetch_one(&mut conn)
        .await?;
        conn.close().await?;
        Ok(u64::try_from(running).unwrap_or(0))
    }
}
