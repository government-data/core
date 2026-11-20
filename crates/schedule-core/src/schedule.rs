//! When a schedule fires next.
//!
//! The expression is standard 5-field cron (`min hour day month weekday`), the
//! same one written in a crontab, evaluated in the schedule's time zone:
//! `0 6 * * 1-5` is "06:00 Bogotá time, Monday to Friday" whatever zone the
//! server runs in. The result comes back in UTC to be stored and compared.
//!
//! Missed fires (the server was down, or the flow or its engine paused)
//! follow the installation's policy (see [`crate::fires_to_run`]); afterwards,
//! the next one is computed from *now*.

use std::str::FromStr;

use chrono::{DateTime, Timelike, Utc};
use chrono_tz::Tz;
use croner::Cron;

/// The default time zone of a schedule.
pub const DEFAULT_TIMEZONE: &str = "America/Bogota";

/// Why a schedule is not valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleError {
    /// The cron expression does not parse.
    Cron(String),
    /// The time zone is not a known IANA name.
    Timezone(String),
    /// The expression never fires again.
    NeverFires,
    /// The trigger is not valid (and why).
    Trigger(String),
}

impl std::fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cron(e) => write!(f, "expresión cron inválida: {e}"),
            Self::Timezone(tz) => write!(f, "zona horaria desconocida {tz:?}"),
            Self::NeverFires => f.write_str("la programación no vuelve a disparar (¿ya pasó?)"),
            Self::Trigger(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for ScheduleError {}

fn parse(expr: &str, timezone: &str) -> Result<(Cron, Tz), ScheduleError> {
    let expr = expr.trim();
    // Exactly 5 fields: `croner` also takes 6 (with seconds), but a flow is
    // not scheduled to the second and an extra field is almost always a copy
    // mistake that would shift everything one position.
    if expr.split_whitespace().count() != 5 {
        return Err(ScheduleError::Cron(
            "se esperan 5 campos: minuto hora día mes día-semana".into(),
        ));
    }
    let cron = Cron::from_str(expr).map_err(|e| ScheduleError::Cron(e.to_string()))?;
    let tz =
        Tz::from_str(timezone.trim()).map_err(|_| ScheduleError::Timezone(timezone.to_string()))?;
    Ok((cron, tz))
}

/// The first fire strictly after `after`, in UTC.
pub fn next_fire(
    expr: &str,
    timezone: &str,
    after: DateTime<Utc>,
) -> Result<DateTime<Utc>, ScheduleError> {
    let (cron, tz) = parse(expr, timezone)?;
    // No fractions of a second: `croner` carries them over from the starting
    // instant, and a fire must land on the exact second (06:00:00, not
    // 06:00:00.908).
    let after = after.with_nanosecond(0).unwrap_or(after);
    cron.find_next_occurrence(&after.with_timezone(&tz), false)
        .map(|local| local.with_timezone(&Utc))
        .map_err(|_| ScheduleError::NeverFires)
}

/// A schedule's next fire: its trigger if it has one (with the random delay
/// already added), otherwise its cron. `None`: it does not fire again (once,
/// or expired). `seed` makes the random delay stable.
pub fn next_schedule_fire(
    cron: &str,
    timezone: &str,
    trigger: Option<&crate::ScheduleTrigger>,
    seed: &str,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, ScheduleError> {
    let Some(t) = trigger else {
        return next_fire(cron, timezone, after).map(Some);
    };
    let tz =
        Tz::from_str(timezone.trim()).map_err(|_| ScheduleError::Timezone(timezone.to_string()))?;
    let after = after.with_nanosecond(0).unwrap_or(after);
    Ok(crate::next_trigger(t, tz, after)
        .map(|next| next + crate::random_delay(seed, next, t.random_delay_secs)))
}

/// Validate a schedule (cron or trigger) before saving it; returns its first
/// fire.
pub fn validate_schedule_spec(
    cron: &str,
    timezone: &str,
    trigger: Option<&crate::ScheduleTrigger>,
    seed: &str,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, ScheduleError> {
    if let Some(t) = trigger {
        crate::validate_trigger(t).map_err(ScheduleError::Trigger)?;
    }
    next_schedule_fire(cron, timezone, trigger, seed, now)?.ok_or(ScheduleError::NeverFires)
}

/// Validate a schedule before saving it; returns its first fire.
pub fn validate_schedule(
    expr: &str,
    timezone: &str,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, ScheduleError> {
    next_fire(expr, timezone, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn evaluates_in_the_schedule_timezone() {
        // 06:00 in Bogotá (UTC-5) is 11:00 UTC.
        let next = next_fire("0 6 * * *", "America/Bogota", utc(2026, 10, 16, 12, 0)).unwrap();
        assert_eq!(next, utc(2026, 10, 17, 11, 0));
    }

    #[test]
    fn fires_on_the_whole_second() {
        let at = utc(2026, 10, 16, 12, 0) + chrono::Duration::milliseconds(908);
        let next = next_fire("* * * * *", "UTC", at).unwrap();
        assert_eq!(next, utc(2026, 10, 16, 12, 1));
        assert_eq!(next.nanosecond(), 0);
    }

    #[test]
    fn next_is_strictly_after() {
        let at = utc(2026, 10, 16, 11, 0);
        let next = next_fire("0 6 * * *", "America/Bogota", at).unwrap();
        assert_eq!(next, utc(2026, 10, 17, 11, 0));
    }

    #[test]
    fn weekdays_skip_the_weekend() {
        // Friday 16 October 2026 at noon → Monday the 19th at 06:00.
        let next = next_fire("0 6 * * 1-5", "America/Bogota", utc(2026, 10, 16, 17, 0)).unwrap();
        assert_eq!(next, utc(2026, 10, 19, 11, 0));
    }

    #[test]
    fn rejects_bad_input() {
        let now = utc(2026, 10, 16, 0, 0);
        assert!(matches!(
            validate_schedule("0 0 6 * * *", "UTC", now),
            Err(ScheduleError::Cron(_))
        ));
        assert!(matches!(
            validate_schedule("61 * * * *", "UTC", now),
            Err(ScheduleError::Cron(_))
        ));
        assert!(matches!(
            validate_schedule("0 6 * * *", "Mars/Olympus", now),
            Err(ScheduleError::Timezone(_))
        ));
    }
}
