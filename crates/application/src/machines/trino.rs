//! Trino's coordinator restarts to take a new certificate only when Trino
//! is free (0066, point 9), checked twice: no job that uses Trino running,
//! the next ones held, and Trino itself running no query. Its agent asks
//! with the certificate in use ([`MachinesService::hold_trino`]); while it is
//! not free nothing is held and it asks again later. Once Trino answers
//! after the restart, it ends the hold with its new certificate
//! ([`MachinesService::release_trino`]); a hold also ends by itself after
//! [`TRINO_HOLD_TTL`], so a dead agent holds nothing for long.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use auth_gateway::{Machine, MachineCertificate, MachineRole};
use domain::AuditEventType;
use domain::AuditEventType::{CertificateRefused, TrinoHeld, TrinoReleased};

use super::{audit, MachinesService, Origin};

/// How long a hold lasts unless released: longer than a restart (about 30
/// seconds) and the agent's wait for Trino (3 minutes), short enough that
/// a dead agent stops the jobs for no longer.
pub const TRINO_HOLD_TTL: Duration = Duration::from_secs(15 * 60);

/// Port: the jobs that use Trino (the server: the queue, in every tenant).
#[async_trait]
pub trait TrinoJobs: Send + Sync {
    /// Holds them for `machine` until `ttl` from now (a hold in force is
    /// extended): none queued is handed out from then on, those running go
    /// on. How many are running.
    async fn hold(&self, machine: &str, ttl: Duration) -> Result<u64, String>;
    /// Ends the hold: whether one was in force.
    async fn release(&self) -> Result<bool, String>;
}

/// Port: Trino's own answer, asked now (the server: its coordinator's
/// `/v1/query` as the API's client, which may view every query).
#[async_trait]
pub trait TrinoQueries: Send + Sync {
    /// How many queries are neither `FINISHED` nor `FAILED`.
    async fn active(&self) -> Result<u64, String>;
}

/// What holding Trino's jobs needs.
#[derive(Clone)]
pub struct TrinoGate {
    pub jobs: Arc<dyn TrinoJobs>,
    pub queries: Arc<dyn TrinoQueries>,
}

impl std::fmt::Debug for TrinoGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrinoGate").finish_non_exhaustive()
    }
}

/// The answer to a coordinator asking whether it may restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrinoHold {
    /// No job that uses Trino running and Trino running no query: the jobs
    /// are held. Otherwise nothing is.
    pub free: bool,
    pub running_jobs: u64,
    /// `None` when Trino did not answer, which is not free.
    pub running_queries: Option<u64>,
}

/// Why a coordinator got no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrinoHoldError {
    /// Not an approved or draining coordinator's certificate.
    Refused,
    /// The API has no Trino or no queue to hold.
    Unavailable,
    Backend(String),
}

impl MachinesService {
    /// With what holding Trino's jobs needs.
    pub fn with_trino(mut self, gate: TrinoGate) -> Self {
        self.trino = Some(gate);
        self
    }

    /// Holds the jobs that use Trino if Trino is free, for the coordinator
    /// of `presented`: first the jobs (held, then counted), then Trino asked.
    /// Not free, the hold is released at once: nothing waits for a
    /// coordinator that may not restart.
    pub async fn hold_trino(
        &self,
        presented: &MachineCertificate,
        origin: &Origin,
    ) -> Result<TrinoHold, TrinoHoldError> {
        let gate = self.trino.clone().ok_or(TrinoHoldError::Unavailable)?;
        let machine = self.coordinator(presented, origin).await?;
        let running_jobs = match gate.jobs.hold(&machine.name, TRINO_HOLD_TTL).await {
            Ok(n) => n,
            Err(e) => {
                release_quietly(&gate).await;
                return Err(TrinoHoldError::Backend(format!("holding: {e}")));
            }
        };
        let running_queries = match gate.queries.active().await {
            Ok(n) => Some(n),
            Err(e) => {
                tracing::warn!("trino did not say whether it runs queries: {e}");
                None
            }
        };
        let free = running_jobs == 0 && running_queries == Some(0);
        if !free {
            release_quietly(&gate).await;
        }
        let action = if free { "trino:held" } else { "trino:busy" };
        self.audit_hold(TrinoHeld, Some(&machine.name), action, origin)
            .await;
        Ok(TrinoHold {
            free,
            running_jobs,
            running_queries,
        })
    }

    /// Ends the hold for the coordinator of `presented` (by then its new
    /// certificate): whether one was in force.
    pub async fn release_trino(
        &self,
        presented: &MachineCertificate,
        origin: &Origin,
    ) -> Result<bool, TrinoHoldError> {
        let gate = self.trino.clone().ok_or(TrinoHoldError::Unavailable)?;
        let machine = self.coordinator(presented, origin).await?;
        let in_force = gate.jobs.release().await.map_err(TrinoHoldError::Backend)?;
        let action = if in_force {
            "trino:released"
        } else {
            "trino:expired"
        };
        self.audit_hold(TrinoReleased, Some(&machine.name), action, origin)
            .await;
        Ok(in_force)
    }

    /// The coordinator of `presented`, or refused and audited.
    async fn coordinator(
        &self,
        presented: &MachineCertificate,
        origin: &Origin,
    ) -> Result<Machine, TrinoHoldError> {
        let found = self
            .caller(presented, MachineRole::TrinoCoordinator)
            .await
            .map_err(|e| TrinoHoldError::Backend(format!("{e:?}")))?;
        match found {
            Some(machine) => Ok(machine),
            None => {
                let name = presented.common_name.as_deref();
                self.audit_hold(CertificateRefused, name, "trino:certificate", origin)
                    .await;
                Err(TrinoHoldError::Refused)
            }
        }
    }

    /// A hold audited (Trino's or Dolt's), or a refusal at the lake's gate,
    /// and answered whether or not it could be.
    pub(super) async fn audit_hold(
        &self,
        event_type: AuditEventType,
        machine: Option<&str>,
        action: &str,
        origin: &Origin,
    ) {
        let event = audit::hold(event_type, machine, action, origin, self.clock.now());
        if let Err(e) = self.record(event).await {
            tracing::error!(error = %e, action, "a hold could not be audited");
        }
    }
}

/// Ends a hold that must not stay; if that fails, it ends by itself.
async fn release_quietly(gate: &TrinoGate) {
    if let Err(e) = gate.jobs.release().await {
        tracing::error!("trino's jobs stay held until the hold expires: {e}");
    }
}
