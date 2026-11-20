//! Progressive lockout: the counters, the pure policy over them, and the
//! per-IP store.

use super::*;

/// The mutable lockout counters for one principal (an account or a client IP).
///
/// It is a plain value object: the transition rules live in [`LockoutPolicy`],
/// and persistence lives behind the [`AdminRepository`] / [`IpLockoutStore`]
/// ports. Keeping the state dumb and the policy pure makes the whole lockout
/// behavior unit-testable without a database or a real clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LockoutState {
    /// Consecutive failed attempts since the last success.
    pub failed_attempts: u32,
    /// When the principal is locked until, if currently locked.
    pub locked_until: Option<SystemTime>,
}

impl LockoutState {
    /// The cleared state: no failures, not locked. Used after a success.
    pub const fn clear() -> Self {
        Self {
            failed_attempts: 0,
            locked_until: None,
        }
    }
}

/// A progressive lockout policy shared by account- and IP-level throttling.
///
/// Failures accumulate; once they reach [`Self::max_attempts`] the principal is
/// locked for a delay that grows with each further failure — doubling from
/// [`Self::base_delay`] and capped at [`Self::max_delay`]. The same pure rules
/// drive both the per-account lock (stored on the admin row) and the per-IP lock
/// (stored keyed by address), so an attacker is throttled whether they spray one
/// account from many IPs or many accounts from one IP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockoutPolicy {
    /// Failures tolerated before the first lock engages.
    pub max_attempts: u32,
    /// The lock duration applied at the threshold; doubles per extra failure.
    pub base_delay: Duration,
    /// The ceiling the doubling delay is clamped to.
    pub max_delay: Duration,
}

impl LockoutPolicy {
    /// A sensible default: lock after 5 failures for 1 minute, up to 1 hour.
    pub const fn recommended() -> Self {
        Self {
            max_attempts: 5,
            base_delay: Duration::from_secs(60),
            max_delay: Duration::from_secs(3600),
        }
    }

    /// Whether `state` is locked at instant `now`.
    pub fn is_locked(&self, state: &LockoutState, now: SystemTime) -> bool {
        matches!(state.locked_until, Some(until) if until > now)
    }

    /// The delay remaining until `state` unlocks at `now`, if locked.
    pub fn retry_after(&self, state: &LockoutState, now: SystemTime) -> Option<Duration> {
        match state.locked_until {
            Some(until) => until.duration_since(now).ok().filter(|d| !d.is_zero()),
            None => None,
        }
    }

    /// Compute the next state after a failed attempt at instant `now`.
    ///
    /// The counter increments; once it reaches `max_attempts` the principal is
    /// locked from `now` for a delay that doubles with each failure past the
    /// threshold, clamped to `max_delay`.
    pub fn register_failure(&self, state: &LockoutState, now: SystemTime) -> LockoutState {
        let failed_attempts = state.failed_attempts.saturating_add(1);
        let locked_until = if failed_attempts >= self.max_attempts.max(1) {
            Some(now + self.lock_duration(failed_attempts))
        } else {
            None
        };
        LockoutState {
            failed_attempts,
            locked_until,
        }
    }

    /// The lock duration for a given failure count: `base * 2^(n - max)` capped
    /// at `max_delay`. At exactly the threshold the delay is `base_delay`.
    fn lock_duration(&self, failed_attempts: u32) -> Duration {
        let threshold = self.max_attempts.max(1);
        let over = failed_attempts.saturating_sub(threshold);
        // Cap the shift so `2^over` cannot overflow; anything beyond saturates to
        // `max_delay` anyway.
        let factor = 1u64.checked_shl(over.min(32)).unwrap_or(u64::MAX);
        let scaled = self
            .base_delay
            .checked_mul(factor.min(u32::MAX as u64) as u32)
            .unwrap_or(self.max_delay);
        scaled.min(self.max_delay)
    }
}

/// Port: persistence for per-IP lockout counters.
///
/// Separate from [`AdminRepository`] because an IP is not an account: it is
/// throttled even when it targets emails that do not exist, which is exactly
/// what defeats account-enumeration-by-lockout.
#[async_trait::async_trait]
pub trait IpLockoutStore: Send + Sync {
    /// The current lockout counters for `ip` (cleared state if never seen).
    async fn get(&self, ip: &str) -> Result<LockoutState, RepositoryError>;

    /// Overwrite the lockout counters for `ip`.
    async fn put(&self, ip: &str, state: &LockoutState) -> Result<(), RepositoryError>;
}
