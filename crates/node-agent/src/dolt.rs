//! Dolt, on the application VM, restarted to take the renewed certificate
//! only when it is free (0066, point 11). The VM's agent runs rolling, as
//! PostgreSQL and Traefik reload what it renews; Dolt reads its certificate
//! only at its start (its entrypoint copies `machine.pem` then), so beside
//! the renewals the agent runs a [`Follower`]: every `QD_HELD_ASK_EVERY`,
//! once the certificate Dolt was last started with is older than
//! `QD_HELD_AFTER` and is not the one in use, it asks the API to hold the
//! catalog ([`ready`]: free only when the API has no operation in flight and
//! Dolt's own list shows nothing running), restarts Dolt (`QD_DOLT_RESTART`),
//! waits until its port answers ([`TcpProbe`]) and releases the hold
//! ([`crate::restart`]). The 24 hours only
//! turn the question on; Dolt says when.
//!
//! What Dolt was started with is the agent's own record, `dolt.followed`
//! in its directory: the SHA-256 of the `machine.pem` it restarted Dolt
//! with, and when. With none (the first run), the one in use is recorded
//! as Dolt's, as Dolt starts after the agent's first certificate.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::api::CallError;
use crate::cert::Credentials;
use crate::ports::Clock;
use crate::restart::{identity, Probe, Release, Restart, RestartError};
use crate::store::FileStore;

/// The API's answer to [`DoltGate::hold`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HoldAnswer {
    pub free: bool,
    /// The catalog's operations in flight in the API.
    pub in_flight: u64,
    /// The statements running in Dolt; `None` when not asked or no answer.
    pub running: Option<u64>,
}

/// The API's two routes for Dolt's restart (0066, step 10), presenting
/// the application VM's certificate (role `app`).
#[async_trait]
pub trait DoltGate: Release {
    /// `POST /internal/machines/dolt/hold`.
    async fn hold(&self, identity: &Credentials, ca: &str) -> Result<HoldAnswer, CallError>;
}

/// Whether Dolt is free, asked with the certificate in use; when it is,
/// the API holds the catalog until [`crate::restart::restart`] releases it.
pub async fn ready(store: &FileStore, gate: &dyn DoltGate) -> Result<bool, String> {
    let (identity, ca) = identity(store)?;
    let answer = gate
        .hold(&identity, &ca)
        .await
        .map_err(|e| format!("hold: {e}"))?;
    if answer.free {
        tracing::info!("Dolt is free: the catalog is held for its restart");
    } else {
        let running = answer.running.map_or("not asked".into(), |n| n.to_string());
        tracing::info!(in_flight = answer.in_flight, running = %running, "Dolt is busy: nothing held, asking again later");
    }
    Ok(answer.free)
}

/// Dolt is back once its port takes a connection: `docker restart`
/// returns once the old process stopped, so the one listening is the new.
#[derive(Debug)]
pub struct TcpProbe {
    pub addr: String,
}

#[async_trait]
impl Probe for TcpProbe {
    async fn back(&self, _since: Duration) -> bool {
        let connect = tokio::net::TcpStream::connect(&self.addr);
        matches!(
            tokio::time::timeout(Duration::from_secs(2), connect).await,
            Ok(Ok(_))
        )
    }
}

/// What a [`Follower`] did at one look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Followed {
    /// No record yet: the certificate in use recorded as Dolt's.
    Recorded,
    /// Dolt was started with the certificate in use.
    Current,
    /// Another, but younger than `QD_HELD_AFTER`: not asked yet.
    NotYet,
    /// Asked, and Dolt is not free (or the API did not answer).
    Busy(String),
    /// Restarted with the certificate in use, the hold released or not.
    Restarted(bool),
    /// The restart failed: the hold is left to expire.
    Failed(RestartError),
}

/// Restarts Dolt when free to give it the certificate in use.
#[derive(Debug)]
pub struct Follower {
    /// `QD_HELD_AFTER`: how old Dolt's certificate may get first.
    pub after: Duration,
    /// `QD_DOLT_RESTART`, its wait and poll.
    pub how: Restart,
}

impl Follower {
    /// One look: ask, restart and record if it is time and Dolt is free.
    pub async fn look(
        &self,
        store: &FileStore,
        gate: &dyn DoltGate,
        probe: &dyn Probe,
        commands: &dyn crate::ports::Commands,
        clock: &dyn Clock,
    ) -> Followed {
        let Ok(Some(current)) = store.current() else {
            return Followed::Busy("no machine.pem yet".into());
        };
        let now = clock.now();
        let hash = digest(&current.bundle());
        let record = record_path(store);
        let Some((followed, at)) = read_record(&record) else {
            return write_or(&record, &hash, now, Followed::Recorded);
        };
        if followed == hash {
            return Followed::Current;
        }
        if now.duration_since(at).unwrap_or_default() < self.after {
            return Followed::NotYet;
        }
        match ready(store, gate).await {
            Ok(true) => {}
            Ok(false) => return Followed::Busy("not free".into()),
            Err(e) => return Followed::Busy(e),
        }
        let restarted = crate::restart::restart(store, gate, probe, commands, clock, &self.how);
        match restarted.await {
            Ok(released) => write_or(&record, &hash, now, Followed::Restarted(released)),
            Err(e) => Followed::Failed(e),
        }
    }
}

fn record_path(store: &FileStore) -> PathBuf {
    store.path("dolt.followed")
}

fn digest(pem: &str) -> String {
    Sha256::digest(pem.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `<sha256> <unix seconds>`.
fn read_record(path: &std::path::Path) -> Option<(String, SystemTime)> {
    let text = std::fs::read_to_string(path).ok()?;
    let (hash, at) = text.trim().split_once(' ')?;
    let at = UNIX_EPOCH + Duration::from_secs(at.parse().ok()?);
    Some((hash.to_string(), at))
}

/// Records `hash` at `now` (written aside and renamed), then `done`; a
/// failure to write is said, as the next look would restart again.
fn write_or(path: &std::path::Path, hash: &str, now: SystemTime, done: Followed) -> Followed {
    let secs = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let aside = path.with_extension("new");
    let written = std::fs::write(&aside, format!("{hash} {secs}\n"))
        .and_then(|()| std::fs::rename(&aside, path));
    match written {
        Ok(()) => done,
        Err(e) => Followed::Failed(RestartError::Files(format!("dolt.followed: {e}"))),
    }
}

#[cfg(test)]
mod tests;
