//! Trino's coordinator restarts to take its next certificate only when
//! Trino is free (0066, point 9), and these are the two commands held mode
//! runs for it, so that no shell script is needed on the machine:
//!
//! - [`ready`] (`qd-node trino-ready`, as `QD_HELD_READY`): asks the API
//!   with the certificate in use whether Trino is free; the API holds the
//!   jobs that use Trino, counts those running and asks Trino itself, and
//!   says `free` only when neither runs anything (not free, it holds
//!   nothing). Exit 0 only when free.
//! - [`restart`] (`qd-node trino-restart`, as `QD_AFTER_RENEW`, run once the
//!   next certificate is in place): restarts the coordinator
//!   (`QD_TRINO_RESTART`), waits until Trino's `/v1/info` says it started
//!   since then, and ends the hold with the new certificate.
//!
//! If Trino does not come back within `QD_TRINO_WAIT` the hold is not
//! released ([`crate::restart`], shared with Dolt's): it ends by itself 15
//! minutes after it was taken (the API's), and the failure is logged.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::api::CallError;
use crate::cert::Credentials;
use crate::ports::{Clock, Commands};
use crate::restart::{identity, Probe, Release};
pub use crate::restart::{Restart, RestartError};
use crate::store::FileStore;

/// The API's answer to [`TrinoGate::hold`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HoldAnswer {
    pub free: bool,
    pub running_jobs: u64,
    /// `None` when Trino did not answer the API.
    pub running_queries: Option<u64>,
}

/// The API's two routes for the coordinator's restart (0066, step 9).
#[async_trait]
pub trait TrinoGate: Send + Sync {
    /// `POST /internal/machines/trino/hold`, presenting `identity`.
    async fn hold(&self, identity: &Credentials, ca: &str) -> Result<HoldAnswer, CallError>;
    /// `POST /internal/machines/trino/release`: whether a hold was in force.
    async fn release(&self, identity: &Credentials, ca: &str) -> Result<bool, CallError>;
}

/// What Trino's `/v1/info` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub starting: bool,
    pub uptime: Duration,
}

/// Trino's `/v1/info`, which needs no token.
#[async_trait]
pub trait TrinoInfo: Send + Sync {
    /// An error when Trino does not answer (it is down or starting).
    async fn info(&self) -> Result<Info, String>;
}

/// Whether Trino is free, asked with the certificate in use; when it is,
/// the API holds its jobs until [`restart`] releases them.
pub async fn ready(store: &FileStore, gate: &dyn TrinoGate) -> Result<bool, String> {
    let (identity, ca) = identity(store)?;
    let answer = gate
        .hold(&identity, &ca)
        .await
        .map_err(|e| format!("hold: {e}"))?;
    let queries = answer
        .running_queries
        .map_or_else(|| "no answer".to_string(), |n| n.to_string());
    if answer.free {
        tracing::info!("Trino is free: its jobs are held for its restart");
    } else {
        tracing::info!(running_jobs = answer.running_jobs, running_queries = %queries, "Trino is busy: nothing held, asking again later");
    }
    Ok(answer.free)
}

/// Restarts the coordinator, waits for Trino to say it started since, and
/// ends the hold with the certificate now in use (the new one). Whether a
/// hold was in force.
pub async fn restart(
    store: &FileStore,
    gate: &dyn TrinoGate,
    info: &dyn TrinoInfo,
    commands: &dyn Commands,
    clock: &dyn Clock,
    how: &Restart,
) -> Result<bool, RestartError> {
    let (gate, probe) = (Trino(gate), Started(info));
    crate::restart::restart(store, &gate, &probe, commands, clock, how).await
}

/// The coordinator's hold, as [`crate::restart`] releases it.
struct Trino<'a>(&'a dyn TrinoGate);

#[async_trait]
impl Release for Trino<'_> {
    async fn release(&self, identity: &Credentials, ca: &str) -> Result<bool, CallError> {
        self.0.release(identity, ca).await
    }
}

/// Trino is back once `/v1/info` says it is not starting and has run for
/// less than the time since the command: not the old process still up.
struct Started<'a>(&'a dyn TrinoInfo);

#[async_trait]
impl Probe for Started<'_> {
    async fn back(&self, since: Duration) -> bool {
        self.0
            .info()
            .await
            .is_ok_and(|now| !now.starting && now.uptime <= since)
    }
}

/// Trino's own form of a duration (`airlift`'s: `12.34s`, `1.50m`,
/// `850.00ms`, `2.00h`, `1.00d`).
pub fn parse_uptime(text: &str) -> Option<Duration> {
    let text = text.trim();
    let split = text.find(|c: char| c.is_ascii_alphabetic())?;
    let (number, unit) = text.split_at(split);
    let number: f64 = number.trim().parse().ok()?;
    let seconds = match unit {
        "ns" => 1e-9,
        "us" => 1e-6,
        "ms" => 1e-3,
        "s" => 1.0,
        "m" => 60.0,
        "h" => 3600.0,
        "d" => 86_400.0,
        _ => return None,
    };
    Duration::try_from_secs_f64(number * seconds).ok()
}

/// `/v1/info`'s body: `starting` and `uptime`.
pub fn read_info(body: &[u8]) -> Option<Info> {
    #[derive(Deserialize)]
    struct Body {
        starting: bool,
        uptime: String,
    }
    let body: Body = serde_json::from_slice(body).ok()?;
    Some(Info {
        starting: body.starting,
        uptime: parse_uptime(&body.uptime)?,
    })
}

#[cfg(test)]
mod tests;
