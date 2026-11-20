//! Schedules (decision 0035): when one fires, what it fires, and which of
//! its fires run after the scheduler was away. Pure: the scheduler stores
//! the next fire and calls these.
//!
//! The expression is a standard 5-field cron (`min hour day month weekday`),
//! read in the schedule's timezone — `0 6 * * 1-5` is 06:00 in Bogotá on
//! weekdays wherever the server runs — and fires come back in UTC, to be
//! stored and compared. Or a trigger (`schedule_core`): once, every N days
//! or weeks, some days of some months, with repetition, a random delay and
//! expiry. The model is the one `knime-orchestador` runs in production.

use std::str::FromStr;

use chrono::{DateTime, Duration, NaiveDate, Timelike, Utc};
use chrono_tz::Tz;
use croner::Cron;
use serde::{Deserialize, Serialize};

pub use schedule_core::ScheduleTrigger;

/// A schedule's timezone when none is given.
pub const DEFAULT_TIMEZONE: &str = "America/Bogota";

/// Late by up to this, a fire is on time.
const ON_TIME: Duration = Duration::seconds(60);

/// Why a schedule cannot be saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleError {
    /// Not five fields.
    Fields,
    /// A field the cron does not understand.
    Cron(String),
    /// Not an IANA timezone.
    Timezone(String),
    /// It never fires again.
    NeverFires,
    /// A trigger that does not hold together (and why).
    Trigger(String),
}

impl std::fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fields => f.write_str("a cron has five fields: minute hour day month weekday"),
            Self::Cron(e) => write!(f, "invalid cron: {e}"),
            Self::Timezone(tz) => write!(f, "unknown timezone {tz:?}"),
            Self::NeverFires => f.write_str("the schedule never fires again"),
            Self::Trigger(e) => write!(f, "invalid trigger: {e}"),
        }
    }
}

impl std::error::Error for ScheduleError {}

/// What a schedule fires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "fires", rename_all = "snake_case")]
pub enum Target {
    /// A cut of a dataset: its extraction, then its evaluation.
    Cut { dataset: String, rerun: Rerun },
    /// A job that needs no cut, queued as it is.
    Job {
        kind: String,
        dataset: Option<String>,
        #[serde(default = "no_parameters")]
        parameters: serde_json::Value,
    },
}

fn no_parameters() -> serde_json::Value {
    serde_json::json!({})
}

/// What firing does when the cut of that date already exists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rerun {
    /// Nothing is created.
    #[default]
    Block,
    /// The cut is extracted and evaluated again.
    Replace,
}

fn parse(expr: &str, timezone: &str) -> Result<(Cron, Tz), ScheduleError> {
    let expr = expr.trim();
    // Exactly five: `croner` also takes six (seconds), but a sixth field is
    // almost always a copy error that shifts every other one.
    if expr.split_whitespace().count() != 5 {
        return Err(ScheduleError::Fields);
    }
    let cron = Cron::from_str(expr).map_err(|e| ScheduleError::Cron(e.to_string()))?;
    let tz = timezone_of(timezone)?;
    Ok((cron, tz))
}

fn timezone_of(timezone: &str) -> Result<Tz, ScheduleError> {
    Tz::from_str(timezone.trim()).map_err(|_| ScheduleError::Timezone(timezone.to_string()))
}

/// The first fire strictly after `after`, in UTC, on the whole second.
pub fn next_fire(
    expr: &str,
    timezone: &str,
    after: DateTime<Utc>,
) -> Result<DateTime<Utc>, ScheduleError> {
    let (cron, tz) = parse(expr, timezone)?;
    // `croner` carries the fraction of the starting instant into the fire.
    let after = after.with_nanosecond(0).unwrap_or(after);
    cron.find_next_occurrence(&after.with_timezone(&tz), false)
        .map(|local| local.with_timezone(&Utc))
        .map_err(|_| ScheduleError::NeverFires)
}

/// When a schedule fires: by its trigger if it has one, else by its cron,
/// both in its timezone. `seed` keeps a trigger's random delay the same
/// each time it is computed.
#[derive(Debug, Clone, Copy)]
pub struct When<'a> {
    pub cron: &'a str,
    pub timezone: &'a str,
    pub trigger: Option<&'a ScheduleTrigger>,
    pub seed: &'a str,
}

impl When<'_> {
    /// The first fire strictly after `after`; `None` if it will not fire
    /// again (a trigger once, or expired).
    pub fn next(&self, after: DateTime<Utc>) -> Result<Option<DateTime<Utc>>, ScheduleError> {
        let Some(trigger) = self.trigger else {
            return next_fire(self.cron, self.timezone, after).map(Some);
        };
        let tz = timezone_of(self.timezone)?;
        let after = after.with_nanosecond(0).unwrap_or(after);
        Ok(schedule_core::next_trigger(trigger, tz, after).map(|next| {
            next + schedule_core::random_delay(self.seed, next, trigger.random_delay_secs)
        }))
    }

    /// Checks a schedule before it is saved; its first fire.
    pub fn validate(&self, now: DateTime<Utc>) -> Result<DateTime<Utc>, ScheduleError> {
        if let Some(trigger) = self.trigger {
            schedule_core::validate_trigger(trigger).map_err(ScheduleError::Trigger)?;
        }
        self.next(now)?.ok_or(ScheduleError::NeverFires)
    }
}

/// Which fire runs for a schedule due at `due`, seen at `now`: on time, that
/// one; late, only the latest it missed, unless even that is older than
/// `missed_max_hours` (decision 0035, point 4). The schedule's next fire is
/// then computed from `now`.
pub fn fire_to_run(
    when: &When<'_>,
    due: DateTime<Utc>,
    now: DateTime<Utc>,
    missed_max_hours: u32,
) -> Option<DateTime<Utc>> {
    if now - due <= ON_TIME {
        return Some(due);
    }
    let mut latest = due;
    // A bound on the walk: every minute for two months.
    for _ in 0..100_000 {
        match when.next(latest) {
            Ok(Some(next)) if next <= now => latest = next,
            _ => break,
        }
    }
    (now - latest <= Duration::hours(i64::from(missed_max_hours))).then_some(latest)
}

/// A cut's date: the day of its fire in the schedule's timezone.
pub fn cut_date(fire: DateTime<Utc>, timezone: &str) -> Result<NaiveDate, ScheduleError> {
    Ok(fire.with_timezone(&timezone_of(timezone)?).date_naive())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn a_cron_is_read_in_its_timezone_and_fires_after_on_the_second() {
        // 06:00 in Bogotá (UTC-5) is 11:00 UTC; strictly after.
        let bogota = "America/Bogota";
        assert_eq!(
            next_fire("0 6 * * *", bogota, utc(2026, 10, 16, 11, 0)).unwrap(),
            utc(2026, 10, 17, 11, 0)
        );
        // Friday noon → Monday 06:00.
        assert_eq!(
            next_fire("0 6 * * 1-5", bogota, utc(2026, 10, 16, 17, 0)).unwrap(),
            utc(2026, 10, 19, 11, 0)
        );
        let at = utc(2026, 10, 16, 12, 0) + Duration::milliseconds(908);
        let next = next_fire("* * * * *", "UTC", at).unwrap();
        assert_eq!((next, next.nanosecond()), (utc(2026, 10, 16, 12, 1), 0));
    }

    fn cron<'a>(cron: &'a str, timezone: &'a str) -> When<'a> {
        When {
            cron,
            timezone,
            trigger: None,
            seed: "",
        }
    }

    fn validate(
        expr: &str,
        timezone: &str,
        now: DateTime<Utc>,
    ) -> Result<DateTime<Utc>, ScheduleError> {
        cron(expr, timezone).validate(now)
    }

    #[test]
    fn a_schedule_that_cannot_fire_is_refused() {
        let now = utc(2026, 10, 16, 0, 0);
        assert_eq!(
            validate("0 0 6 * * *", "UTC", now),
            Err(ScheduleError::Fields)
        );
        assert!(matches!(
            validate("61 * * * *", "UTC", now),
            Err(ScheduleError::Cron(_))
        ));
        assert!(matches!(
            validate("0 6 * * *", "Mars/Olympus", now),
            Err(ScheduleError::Timezone(_))
        ));
        assert_eq!(
            validate("0 0 30 2 *", "UTC", now),
            Err(ScheduleError::NeverFires)
        );
    }

    #[test]
    fn after_an_absence_only_the_latest_missed_fire_runs() {
        let when = cron("0 6 * * *", "America/Bogota");
        let due = utc(2026, 10, 16, 11, 0);
        // On time, or within the minute.
        assert_eq!(
            fire_to_run(&when, due, due + Duration::seconds(30), 24),
            Some(due)
        );
        // Three days away: the third morning only.
        let now = utc(2026, 10, 18, 15, 0);
        assert_eq!(
            fire_to_run(&when, due, now, 24),
            Some(utc(2026, 10, 18, 11, 0))
        );
        // Nothing missed recently enough.
        assert_eq!(fire_to_run(&when, due, now, 2), None);
    }

    fn trigger(json: serde_json::Value) -> ScheduleTrigger {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_trigger_fires_by_itself_and_once_is_once() {
        let every_two_weeks = trigger(serde_json::json!({
            "start": "2026-10-19T06:00:00",
            "recurrence": {"kind": "weekly", "every": 2, "days": [1]},
        }));
        let when = When {
            cron: "",
            timezone: "America/Bogota",
            trigger: Some(&every_two_weeks),
            seed: "loans",
        };
        // Monday the 19th at 06:00 Bogotá, then two weeks later.
        let first = when.validate(utc(2026, 10, 16, 0, 0)).unwrap();
        assert_eq!(first, utc(2026, 10, 19, 11, 0));
        assert_eq!(when.next(first).unwrap(), Some(utc(2026, 11, 2, 11, 0)));
        // Three weeks away: only the latest missed, the one of the 2nd.
        let now = utc(2026, 11, 9, 0, 0);
        assert_eq!(
            fire_to_run(&when, first, now, 24 * 30),
            Some(utc(2026, 11, 2, 11, 0))
        );

        let once = trigger(serde_json::json!({
            "start": "2026-10-19T06:00:00", "recurrence": {"kind": "once"},
        }));
        let when = When {
            trigger: Some(&once),
            ..when
        };
        let first = when.validate(utc(2026, 10, 16, 0, 0)).unwrap();
        assert_eq!(when.next(first).unwrap(), None);
        // Past already: refused when saved.
        assert_eq!(
            when.validate(utc(2026, 10, 20, 0, 0)),
            Err(ScheduleError::NeverFires)
        );
        let none = trigger(serde_json::json!({
            "start": "2026-10-19T06:00:00",
            "recurrence": {"kind": "weekly", "every": 1, "days": []},
        }));
        assert!(matches!(
            When {
                trigger: Some(&none),
                ..when
            }
            .validate(utc(2026, 10, 16, 0, 0)),
            Err(ScheduleError::Trigger(_))
        ));
    }

    #[test]
    fn a_cut_is_dated_in_the_schedules_timezone() {
        // 02:00 UTC on the 17th is still the 16th in Bogotá.
        let fire = utc(2026, 10, 17, 2, 0);
        assert_eq!(
            cut_date(fire, "America/Bogota").unwrap(),
            NaiveDate::from_ymd_opt(2026, 10, 16).unwrap()
        );
        assert_eq!(
            cut_date(fire, "UTC").unwrap(),
            NaiveDate::from_ymd_opt(2026, 10, 17).unwrap()
        );
    }

    #[test]
    fn a_target_is_a_cut_or_a_job_without_one() {
        let cut: Target = serde_json::from_value(
            serde_json::json!({"fires": "cut", "dataset": "loans", "rerun": "replace"}),
        )
        .unwrap();
        assert_eq!(
            cut,
            Target::Cut {
                dataset: "loans".into(),
                rerun: Rerun::Replace
            }
        );
        let job: Target = serde_json::from_value(
            serde_json::json!({"fires": "job", "kind": "maintain", "dataset": null}),
        )
        .unwrap();
        assert_eq!(
            job,
            Target::Job {
                kind: "maintain".into(),
                dataset: None,
                parameters: serde_json::json!({})
            }
        );
    }
}
