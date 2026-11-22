//! Firing schedules (decision 0035): one transaction creates what a due
//! schedule fires — a cut and its extraction, or a job without a cut — and
//! advances its next fire. It runs as the application, inside the tenant's
//! `tenancy::within`, so row security keeps it to that tenant; due rows are
//! taken `FOR UPDATE SKIP LOCKED`, so two schedulers never fire one twice.

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::schedule::{self, Rerun, ScheduleTrigger, When};

/// What a fire did, as `schedules.last_outcome` keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Its cut or its job was created.
    Queued,
    /// Its cut already existed and the schedule blocks a re-run, or it is
    /// still running.
    Blocked,
    /// Its cut existed and is being extracted again.
    Replaced,
    /// Due too long ago (`missed_max_hours`): nothing was created.
    Skipped,
}

impl Outcome {
    /// As `schedules.last_outcome` and _Run now_'s answer say it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Blocked => "blocked",
            Self::Replaced => "replaced",
            Self::Skipped => "skipped",
        }
    }
}

/// One schedule fired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fired {
    pub schedule: Uuid,
    pub outcome: Outcome,
    /// When it was due, and the fire that ran (`None` if skipped).
    pub due_at: DateTime<Utc>,
    pub fire: Option<DateTime<Utc>>,
    pub cut: Option<Uuid>,
    pub job: Option<Uuid>,
}

#[derive(sqlx::FromRow)]
struct Due {
    id: Uuid,
    cron: String,
    trigger: Option<sqlx::types::Json<ScheduleTrigger>>,
    timezone: String,
    fires: String,
    dataset: Option<String>,
    rerun: String,
    kind: Option<String>,
    parameters: serde_json::Value,
    next_fire_at: DateTime<Utc>,
}

impl Due {
    /// As [`crate::schedule_store::ScheduleSpec::when`] says it.
    fn when(&self) -> When<'_> {
        When {
            cron: &self.cron,
            timezone: &self.timezone,
            trigger: self.trigger.as_deref(),
            seed: self.dataset.as_deref().unwrap_or_default(),
        }
    }
}

/// Fires this tenant's schedules due at `now`.
pub async fn fire_due(
    pool: &PgPool,
    now: DateTime<Utc>,
    missed_max_hours: u32,
) -> Result<Vec<Fired>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let due: Vec<Due> = sqlx::query_as(
        "SELECT id, cron, trigger, timezone, fires, dataset, rerun, kind, parameters, next_fire_at
           FROM quality.schedules
          WHERE enabled AND next_fire_at <= $1
          FOR UPDATE SKIP LOCKED",
    )
    .bind(now)
    .fetch_all(&mut *tx)
    .await?;
    let mut fired = Vec::with_capacity(due.len());
    for s in due {
        // Validated when saved; a timezone retired since turns it off. A
        // trigger that will not fire again keeps no next fire.
        let when = s.when();
        let Ok(next) = when.next(now) else {
            tracing::warn!(schedule = %s.id, "schedule no longer fires: disabled");
            sqlx::query("UPDATE quality.schedules SET enabled = false WHERE id = $1")
                .bind(s.id)
                .execute(&mut *tx)
                .await?;
            continue;
        };
        let fire = schedule::fire_to_run(&when, s.next_fire_at, now, missed_max_hours);
        let (outcome, cut, job) = match fire {
            None => (Outcome::Skipped, None, None),
            Some(fire) => fire_one(&mut tx, &s, fire).await?,
        };
        if fire != Some(s.next_fire_at) {
            tracing::info!(schedule = %s.id, due = %s.next_fire_at, outcome = outcome.as_str(), "late schedule");
        }
        sqlx::query(
            "UPDATE quality.schedules
                SET next_fire_at = $2, last_fired_at = COALESCE($3, last_fired_at), last_outcome = $4
              WHERE id = $1",
        )
        .bind(s.id)
        .bind(next)
        .bind(fire)
        .bind(outcome.as_str())
        .execute(&mut *tx)
        .await?;
        fired.push(Fired {
            schedule: s.id,
            outcome,
            due_at: s.next_fire_at,
            fire,
            cut,
            job,
        });
    }
    tx.commit().await?;
    Ok(fired)
}

async fn fire_one(
    tx: &mut Transaction<'_, Postgres>,
    s: &Due,
    fire: DateTime<Utc>,
) -> Result<(Outcome, Option<Uuid>, Option<Uuid>), sqlx::Error> {
    match (s.fires.as_str(), &s.dataset, &s.kind) {
        ("cut", Some(dataset), _) => {
            let Ok(date) = schedule::cut_date(fire, &s.timezone) else {
                return Ok((Outcome::Skipped, None, None));
            };
            let rerun = if s.rerun == "replace" {
                Rerun::Replace
            } else {
                Rerun::Block
            };
            let request = CutRequest {
                dataset,
                date,
                rerun,
                schedule: Some(s.id),
                requested_by: None,
                due_at: fire,
            };
            open_cut(tx, &request).await
        }
        (_, dataset, Some(kind)) => {
            let job = queue_job(tx, kind, dataset.as_deref(), &s.parameters).await?;
            Ok((Outcome::Queued, None, Some(job)))
        }
        // The table's CHECK keeps this from happening.
        _ => Ok((Outcome::Skipped, None, None)),
    }
}

/// A cut to open: from a schedule's fire or a person's _Run now_.
#[derive(Debug, Clone)]
pub struct CutRequest<'a> {
    pub dataset: &'a str,
    pub date: NaiveDate,
    pub rerun: Rerun,
    pub schedule: Option<Uuid>,
    pub requested_by: Option<Uuid>,
    pub due_at: DateTime<Utc>,
}

/// Opens the dataset's cut of that date with its extraction; if it exists,
/// replaces it when asked to and it is not running, else blocks.
pub async fn open_cut(
    tx: &mut Transaction<'_, Postgres>,
    r: &CutRequest<'_>,
) -> Result<(Outcome, Option<Uuid>, Option<Uuid>), sqlx::Error> {
    let created: Option<(Uuid, i32)> = sqlx::query_as(
        "INSERT INTO quality.cuts (dataset, cut, schedule_id, requested_by, due_at)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (tenant_id, dataset, cut) DO NOTHING RETURNING id, run",
    )
    .bind(r.dataset)
    .bind(r.date)
    .bind(r.schedule)
    .bind(r.requested_by)
    .bind(r.due_at)
    .fetch_optional(&mut **tx)
    .await?;
    let (outcome, (cut, run)) = match created {
        Some(cut) => (Outcome::Queued, cut),
        None if r.rerun == Rerun::Replace => {
            // A cut still extracting or evaluating is not replaced under it.
            let replaced: Option<(Uuid, i32)> = sqlx::query_as(
                "UPDATE quality.cuts
                    SET run = run + 1, state = 'extracting', evaluate_job = NULL,
                        bronze_snapshot = NULL, schedule_id = $3, requested_by = $4,
                        due_at = $5, fired_at = now()
                  WHERE dataset = $1 AND cut = $2 AND state IN ('done', 'failed')
                  RETURNING id, run",
            )
            .bind(r.dataset)
            .bind(r.date)
            .bind(r.schedule)
            .bind(r.requested_by)
            .bind(r.due_at)
            .fetch_optional(&mut **tx)
            .await?;
            match replaced {
                Some(cut) => (Outcome::Replaced, cut),
                None => return Ok((Outcome::Blocked, None, None)),
            }
        }
        None => return Ok((Outcome::Blocked, None, None)),
    };
    // The cut's id and replacement number: the logs' run_id and run (0026).
    let parameters = serde_json::json!({ "cut": r.date.to_string(), "cut_id": cut, "run": run });
    let job = queue_job(tx, "extract", Some(r.dataset), &parameters).await?;
    sqlx::query("UPDATE quality.cuts SET extract_job = $2 WHERE id = $1")
        .bind(cut)
        .bind(job)
        .execute(&mut **tx)
        .await?;
    Ok((outcome, Some(cut), Some(job)))
}

/// Queues a `kind` job. `uses_trino` is no longer written (decision
/// 0077): it stays NULL until a later version drops it.
pub(crate) async fn queue_job(
    tx: &mut Transaction<'_, Postgres>,
    kind: &str,
    dataset: Option<&str>,
    parameters: &serde_json::Value,
) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO quality.jobs (kind, dataset, parameters)
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(kind)
    .bind(dataset)
    .bind(parameters)
    .fetch_one(&mut **tx)
    .await
}

/// Marks failed the tenant's cuts whose extraction or evaluation failed
/// for good (decision 0035, point 3): the queue fails the job, and its cut
/// follows on the scheduler's next round. How many it marked.
pub async fn fail_cuts(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE quality.cuts c SET state = 'failed'
           FROM quality.jobs j
          WHERE j.tenant_id = c.tenant_id AND j.state = 'failed'
            AND ((c.state = 'extracting' AND j.id = c.extract_job)
              OR (c.state = 'evaluating' AND j.id = c.evaluate_job))",
    )
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}
