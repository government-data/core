//! What the agent does next, decided from what it holds and the time, with
//! no I/O: so every case is a unit test.

use std::time::{Duration, SystemTime};

use crate::cert::Validity;

/// How long a failed call waits before the first retry; doubled each time.
pub const FIRST_BACKOFF: Duration = Duration::from_secs(5);
/// The most a failed call waits.
pub const MAX_BACKOFF: Duration = Duration::from_secs(600);
/// The longest wait between two looks, so a clock that jumps is seen.
pub const LONGEST_WAIT: Duration = Duration::from_secs(3600);

/// How a renewed certificate is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// At once: the services reload it (the default).
    Rolling,
    /// Only when the service may be restarted (Trino's coordinator, Dolt:
    /// 0066, points 9 and 11): asked once the certificate in use is older
    /// than `after`, every `ask_every`.
    Held {
        after: Duration,
        ask_every: Duration,
    },
}

/// What the agent knows at a moment.
#[derive(Debug, Clone, Copy)]
pub struct Facts {
    pub now: SystemTime,
    /// The certificate in use; `None` before joining.
    pub current: Option<Validity>,
    /// A renewed certificate not yet in use.
    pub next: Option<Validity>,
    /// When to call the API next (join or renew).
    pub call_at: SystemTime,
    /// When to ask, in held mode, whether the next may be taken.
    pub ask_at: SystemTime,
}

/// What to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Join,
    /// Take the next certificate and run the hook.
    Promote,
    Renew,
    /// Ask whether the next certificate may be taken now.
    AskReady,
    /// The certificate ended and there is no other: only a new join helps.
    Ended,
    Wait(Duration),
}

pub fn decide(mode: Mode, f: &Facts) -> Step {
    let next = f.next.filter(|n| !n.ended(f.now));
    let Some(current) = f.current else {
        return match (next, f.now >= f.call_at) {
            (Some(_), _) => Step::Promote,
            (None, true) => Step::Join,
            (None, false) => wait(f, &[f.call_at]),
        };
    };
    if current.ended(f.now) {
        // Nothing works with it any more: a held service is not waited for.
        return if next.is_some() {
            Step::Promote
        } else {
            Step::Ended
        };
    }
    let mut moments = vec![f.call_at, current.not_after];
    match (mode, next) {
        (Mode::Rolling, Some(_)) => return Step::Promote,
        (Mode::Held { after, .. }, Some(_)) => {
            let asking_from = current.not_before + after;
            if f.now >= f.call_at {
                return Step::Renew;
            }
            if f.now >= asking_from && f.now >= f.ask_at {
                return Step::AskReady;
            }
            moments.push(asking_from.max(f.ask_at));
        }
        _ if f.now >= f.call_at => return Step::Renew,
        _ => {}
    }
    wait(f, &moments)
}

fn wait(f: &Facts, moments: &[SystemTime]) -> Step {
    let until = moments.iter().min().copied().unwrap_or(f.now);
    let wait = until.duration_since(f.now).unwrap_or_default();
    Step::Wait(wait.clamp(Duration::from_secs(1), LONGEST_WAIT))
}

/// When to renew first, at start: at once if under two thirds of the
/// certificate's life is left, else after one period.
pub fn first_renewal(now: SystemTime, current: &Validity, period: Duration) -> SystemTime {
    if current.remaining(now) < current.lifetime() * 2 / 3 {
        now
    } else {
        now + period
    }
}

/// `every` moved by up to ±10%: `unit` is in [-1, 1].
pub fn jittered(every: Duration, unit: f64) -> Duration {
    let unit = if unit.is_finite() {
        unit.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    every.mul_f64(1.0 + unit / 10.0)
}

/// How long to wait after `failures` calls failed in a row (one or more).
pub fn backoff(failures: u32) -> Duration {
    let doubled = 2u32.saturating_pow(failures.saturating_sub(1).min(16));
    FIRST_BACKOFF.saturating_mul(doubled).min(MAX_BACKOFF)
}

#[cfg(test)]
mod tests;
