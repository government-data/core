//! The queue as a worker sees it (decisions 0018, 0023, 0027): take the
//! next job, renew its lease while working, finish it. Connected as the
//! queue's role, which can do only that; the SQL is the module's
//! (migrations/0002_queue.sql), these are its calls. A session opened with a
//! machine's certificate is that machine: claiming fails unless it is
//! approved, renewing and finishing unless it is approved or draining, with
//! the reason in the error (migrations/0024_queue_machines.sql).

use std::time::Duration;

use sqlx::postgres::types::PgInterval;
use sqlx::PgPool;
use uuid::Uuid;

/// How long a lease lasts and how long a failed job waits before its next
/// attempt (doubling with each attempt).
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub lease: Duration,
    pub backoff: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            lease: Duration::from_secs(60),
            backoff: Duration::from_secs(60),
        }
    }
}

/// A job this worker holds until it finishes it or its lease runs out.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ClaimedJob {
    pub tenant_id: Uuid,
    pub id: Uuid,
    pub kind: String,
    /// `None` only for a job of the whole warehouse (`maintain`).
    pub dataset: Option<String>,
    pub parameters: serde_json::Value,
    pub engine: String,
    pub attempts: i32,
}

fn interval(d: Duration) -> PgInterval {
    PgInterval {
        months: 0,
        days: 0,
        microseconds: i64::try_from(d.as_micros()).unwrap_or(i64::MAX),
    }
}

/// One worker's handle on the queue.
#[derive(Debug, Clone)]
pub struct Queue {
    pool: PgPool,
    worker: String,
    timing: Timing,
}

impl Queue {
    /// `pool` connects as the queue's role; `worker` names this worker
    /// (its slot) in the jobs it holds.
    pub fn new(pool: PgPool, worker: impl Into<String>, timing: Timing) -> Self {
        Self {
            pool,
            worker: worker.into(),
            timing,
        }
    }

    /// The next ready job of these kinds, if any. Jobs whose lease ran out
    /// go back to the queue first.
    pub async fn claim(&self, kinds: &[&str]) -> Result<Option<ClaimedJob>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM quality.claim_job($1, $2, $3, $4)")
            .bind(&self.worker)
            .bind(kinds)
            .bind(interval(self.timing.lease))
            .bind(interval(self.timing.backoff))
            .fetch_optional(&self.pool)
            .await
    }

    /// Extends the lease; `false` if this worker no longer holds the job.
    pub async fn renew(&self, job: &ClaimedJob) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar("SELECT quality.renew_job($1, $2, $3, $4)")
            .bind(job.tenant_id)
            .bind(job.id)
            .bind(&self.worker)
            .bind(interval(self.timing.lease))
            .fetch_one(&self.pool)
            .await
    }

    /// Finishes the job: done with no error, otherwise back to the queue
    /// while it has attempts. `false` if this worker no longer holds it.
    pub async fn finish(&self, job: &ClaimedJob, error: Option<&str>) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar("SELECT quality.finish_job($1, $2, $3, $4, $5)")
            .bind(job.tenant_id)
            .bind(job.id)
            .bind(&self.worker)
            .bind(error)
            .bind(interval(self.timing.backoff))
            .fetch_one(&self.pool)
            .await
    }
}
