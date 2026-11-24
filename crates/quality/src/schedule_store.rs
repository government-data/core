//! The schedules (decision 0035), the module's own adapter as the queue
//! is. A schedule is saved
//! only if it holds together ([`ScheduleSpec::check`]: a cron of five
//! fields, or a trigger, that fires again in its timezone, and a target the
//! table can queue), with its next fire computed here, as the scheduler computes it.
//! A schedule with cuts is disabled, never deleted: its cuts keep where they
//! came from.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::schedule::{Rerun, ScheduleError, ScheduleTrigger, Target, When, DEFAULT_TIMEZONE};

/// The kinds of job a schedule may fire without a cut.
pub const JOB_KINDS: &[&str] = &["map", "maintain"];

/// A schedule as a person writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleSpec {
    /// Five fields: minute hour day month weekday. Empty with a trigger.
    #[serde(default)]
    pub cron: String,
    /// When set, the schedule fires by it, not by `cron`.
    #[serde(default)]
    pub trigger: Option<ScheduleTrigger>,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(flatten)]
    pub target: Target,
}

fn default_timezone() -> String {
    DEFAULT_TIMEZONE.to_string()
}

fn enabled() -> bool {
    true
}

/// Why a schedule is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecError {
    Schedule(ScheduleError),
    /// Not one of [`JOB_KINDS`].
    Kind,
    /// A cut and every job need their dataset (a `maintain` too, 0039).
    Dataset,
}

impl ScheduleSpec {
    /// Its first fire after `now`, if it holds together.
    pub fn check(&self, now: DateTime<Utc>) -> Result<DateTime<Utc>, SpecError> {
        match &self.target {
            Target::Cut { dataset, .. } if dataset.trim().is_empty() => {
                return Err(SpecError::Dataset)
            }
            Target::Job { kind, .. } if !JOB_KINDS.contains(&kind.as_str()) => {
                return Err(SpecError::Kind)
            }
            // A maintenance is a dataset's too (decision 0039).
            Target::Job { dataset: None, .. } => return Err(SpecError::Dataset),
            _ => {}
        }
        self.when().validate(now).map_err(SpecError::Schedule)
    }

    /// When it fires; a trigger's random delay seeded by its dataset.
    pub fn when(&self) -> When<'_> {
        When {
            cron: &self.cron,
            timezone: &self.timezone,
            trigger: self.trigger.as_ref(),
            seed: self.dataset().unwrap_or_default(),
        }
    }

    /// The cron stored: none with a trigger.
    fn stored_cron(&self) -> &str {
        if self.trigger.is_some() {
            ""
        } else {
            self.cron.trim()
        }
    }

    /// The dataset it is about, if any.
    pub fn dataset(&self) -> Option<&str> {
        match &self.target {
            Target::Cut { dataset, .. } => Some(dataset),
            Target::Job { dataset, .. } => dataset.as_deref(),
        }
    }
}

/// A stored schedule, with what the scheduler keeps of it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StoredSchedule {
    pub id: Uuid,
    #[serde(flatten)]
    pub spec: ScheduleSpec,
    pub next_fire_at: Option<DateTime<Utc>>,
    pub last_fired_at: Option<DateTime<Utc>>,
    /// `queued`, `blocked`, `replaced` or `skipped`.
    pub last_outcome: Option<String>,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    cron: String,
    trigger: Option<sqlx::types::Json<ScheduleTrigger>>,
    timezone: String,
    enabled: bool,
    fires: String,
    dataset: Option<String>,
    rerun: String,
    kind: Option<String>,
    parameters: serde_json::Value,
    next_fire_at: Option<DateTime<Utc>>,
    last_fired_at: Option<DateTime<Utc>>,
    last_outcome: Option<String>,
}

const COLUMNS: &str =
    "id, cron, trigger, timezone, enabled, fires, dataset, rerun, kind, parameters, \
                       next_fire_at, last_fired_at, last_outcome";

impl From<Row> for StoredSchedule {
    fn from(r: Row) -> Self {
        let target = match (r.fires.as_str(), r.dataset, r.kind) {
            ("cut", Some(dataset), _) => Target::Cut {
                dataset,
                rerun: if r.rerun == "replace" {
                    Rerun::Replace
                } else {
                    Rerun::Block
                },
            },
            (_, dataset, kind) => Target::Job {
                kind: kind.unwrap_or_default(),
                dataset,
                parameters: r.parameters,
            },
        };
        Self {
            id: r.id,
            spec: ScheduleSpec {
                cron: r.cron,
                trigger: r.trigger.map(|t| t.0),
                timezone: r.timezone,
                enabled: r.enabled,
                target,
            },
            next_fire_at: r.next_fire_at,
            last_fired_at: r.last_fired_at,
            last_outcome: r.last_outcome,
        }
    }
}

/// The columns a spec writes: fires, dataset, rerun, kind, parameters.
fn columns(
    spec: &ScheduleSpec,
) -> (
    &'static str,
    Option<&str>,
    &'static str,
    Option<&str>,
    serde_json::Value,
) {
    match &spec.target {
        Target::Cut { dataset, rerun } => (
            "cut",
            Some(dataset),
            if *rerun == Rerun::Replace {
                "replace"
            } else {
                "block"
            },
            None,
            serde_json::json!({}),
        ),
        Target::Job {
            kind,
            dataset,
            parameters,
        } => (
            "job",
            dataset.as_deref(),
            "block",
            Some(kind),
            parameters.clone(),
        ),
    }
}

/// The schedules, oldest first.
pub async fn list(pool: &PgPool) -> Result<Vec<StoredSchedule>, sqlx::Error> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM quality.schedules ORDER BY created_at, id"
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(StoredSchedule::from).collect())
}

/// The schedule `id`, if there is one.
pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<StoredSchedule>, sqlx::Error> {
    let row: Option<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM quality.schedules WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(StoredSchedule::from))
}

/// The platform's time zone (decision 0079): what it saved in Settings, or
/// UTC when it saved none or one that does not parse.
pub async fn platform_timezone(pool: &PgPool) -> Result<String, sqlx::Error> {
    let saved: Option<String> = sqlx::query_scalar("SELECT public.platform_timezone()")
        .fetch_one(pool)
        .await?;
    Ok(schedule_core::timezone_or_utc(saved.as_deref()))
}

/// Saves a new schedule, already checked, with its first fire.
pub async fn create(
    pool: &PgPool,
    spec: &ScheduleSpec,
    next_fire: DateTime<Utc>,
) -> Result<Uuid, sqlx::Error> {
    let (fires, dataset, rerun, kind, parameters) = columns(spec);
    sqlx::query_scalar(
        "INSERT INTO quality.schedules
             (cron, timezone, enabled, fires, dataset, rerun, kind, parameters, next_fire_at,
              trigger)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id",
    )
    .bind(spec.stored_cron())
    .bind(spec.timezone.trim())
    .bind(spec.enabled)
    .bind(fires)
    .bind(dataset)
    .bind(rerun)
    .bind(kind)
    .bind(parameters)
    .bind(next_fire)
    .bind(spec.trigger.as_ref().map(sqlx::types::Json))
    .fetch_one(pool)
    .await
}

/// Replaces schedule `id`, already checked; its next fire from now. `false`
/// if it is not there.
pub async fn update(
    pool: &PgPool,
    id: Uuid,
    spec: &ScheduleSpec,
    next_fire: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    let (fires, dataset, rerun, kind, parameters) = columns(spec);
    let done = sqlx::query(
        "UPDATE quality.schedules
            SET cron = $2, timezone = $3, enabled = $4, fires = $5, dataset = $6, rerun = $7,
                kind = $8, parameters = $9, next_fire_at = $10, trigger = $11
          WHERE id = $1",
    )
    .bind(id)
    .bind(spec.stored_cron())
    .bind(spec.timezone.trim())
    .bind(spec.enabled)
    .bind(fires)
    .bind(dataset)
    .bind(rerun)
    .bind(kind)
    .bind(parameters)
    .bind(next_fire)
    .bind(spec.trigger.as_ref().map(sqlx::types::Json))
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// What deleting a schedule did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deleted {
    Deleted,
    NotFound,
    /// It has cuts: disable it instead.
    HasCuts,
}

/// Deletes schedule `id` unless it has cuts.
pub async fn delete(pool: &PgPool, id: Uuid) -> Result<Deleted, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let found: Option<bool> = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM quality.cuts WHERE schedule_id = s.id)
           FROM quality.schedules s WHERE s.id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let outcome = match found {
        None => Deleted::NotFound,
        Some(true) => Deleted::HasCuts,
        Some(false) => {
            sqlx::query("DELETE FROM quality.schedules WHERE id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            Deleted::Deleted
        }
    };
    tx.commit().await?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn spec(json: serde_json::Value) -> ScheduleSpec {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_schedule_holds_together_or_says_why() {
        let now = Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap();
        let cut = spec(serde_json::json!({
            "cron": "0 6 * * *", "fires": "cut", "dataset": "loans", "rerun": "block"
        }));
        assert_eq!(cut.timezone, DEFAULT_TIMEZONE);
        assert!(cut.enabled);
        // 06:00 in Bogotá is 11:00 UTC: the next one is tomorrow's.
        assert_eq!(
            cut.check(now).unwrap(),
            Utc.with_ymd_and_hms(2026, 11, 1, 11, 0, 0).unwrap()
        );
        let maintain = spec(serde_json::json!({
            "cron": "0 2 * * 0", "fires": "job", "kind": "maintain", "dataset": "loans"
        }));
        assert!(maintain.check(now).is_ok());
        // A maintenance is a dataset's (decision 0039).
        let warehouse =
            spec(serde_json::json!({ "cron": "0 2 * * 0", "fires": "job", "kind": "maintain" }));
        assert_eq!(warehouse.check(now).unwrap_err(), SpecError::Dataset);
        for (bad, why) in [
            (
                serde_json::json!({ "cron": "0 6 * *", "fires": "cut", "dataset": "loans", "rerun": "block" }),
                SpecError::Schedule(ScheduleError::Fields),
            ),
            (
                serde_json::json!({ "cron": "0 6 * * *", "fires": "job", "kind": "evaluate", "dataset": "loans" }),
                SpecError::Kind,
            ),
            (
                serde_json::json!({ "cron": "0 6 * * *", "fires": "job", "kind": "map" }),
                SpecError::Dataset,
            ),
            (
                serde_json::json!({ "cron": "0 6 * * *", "timezone": "Mars/Base", "fires": "cut", "dataset": "loans", "rerun": "block" }),
                SpecError::Schedule(ScheduleError::Timezone("Mars/Base".into())),
            ),
        ] {
            assert_eq!(spec(bad).check(now).unwrap_err(), why);
        }
    }
}
