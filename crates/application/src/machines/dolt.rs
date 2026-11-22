//! Dolt restarts to take a renewed certificate only when it is free (0066,
//! point 11), checked twice: the API, Dolt's only client, has no operation of the catalog in flight and
//! holds the next ones, and Dolt's own list shows no other connection
//! running a statement. The application VM's agent asks with its
//! certificate (role `app`, point 15) ([`MachinesService::hold_dolt`]);
//! while Dolt is not free nothing is held and it asks again later. Once
//! Dolt answers after the restart, it ends the hold
//! ([`MachinesService::release_dolt`]); a hold also ends by itself after
//! [`DOLT_HOLD_TTL`], so a dead agent holds the catalog for no longer.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use auth_gateway::{Machine, MachineCertificate, MachineRole};
use domain::AuditEventType::{CertificateRefused, DoltHeld, DoltReleased};

use super::{MachinesService, Origin};

/// How long a hold lasts unless released: Dolt is back in about 3 seconds
/// and the agent waits for it at most 2 minutes; the catalog's operations
/// wait 30 seconds each, then fail.
pub const DOLT_HOLD_TTL: Duration = Duration::from_secs(5 * 60);

/// Port: the catalog's Dolt as this API uses it (the server: the Dolt
/// adapter, the one every module's catalog goes through).
#[async_trait]
pub trait DoltStore: Send + Sync {
    /// Holds the catalog for `ttl` if Dolt is free; not free, nothing stays
    /// held.
    async fn hold(&self, ttl: Duration) -> DoltHold;
    /// Ends the hold: whether one was in force.
    fn release(&self) -> bool;
}

/// The answer to the application VM's agent asking whether Dolt may
/// restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoltHold {
    /// No operation of the catalog in flight and nothing running in Dolt:
    /// held. Otherwise nothing is.
    pub free: bool,
    /// The catalog's operations in flight in the API.
    pub in_flight: u64,
    /// The other connections running a statement in Dolt; `None` when Dolt
    /// was not asked or did not answer, which is not free.
    pub running: Option<u64>,
}

/// Why the agent got no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DoltHoldError {
    /// Not an approved or draining `app` machine's certificate.
    Refused,
    /// The API has no catalog.
    Unavailable,
    Backend(String),
}

impl MachinesService {
    /// With the catalog's Dolt, for its restart.
    pub fn with_dolt(mut self, dolt: Arc<dyn DoltStore>) -> Self {
        self.dolt = Some(dolt);
        self
    }

    /// Holds the catalog if Dolt is free, for the application VM's machine
    /// of `presented`.
    pub async fn hold_dolt(
        &self,
        presented: &MachineCertificate,
        origin: &Origin,
    ) -> Result<DoltHold, DoltHoldError> {
        let dolt = self.dolt.clone().ok_or(DoltHoldError::Unavailable)?;
        let machine = self.app_machine(presented, origin).await?;
        let hold = dolt.hold(DOLT_HOLD_TTL).await;
        let action = if hold.free { "dolt:held" } else { "dolt:busy" };
        self.audit_hold(DoltHeld, Some(&machine.name), action, origin)
            .await;
        Ok(hold)
    }

    /// Ends the hold for the machine of `presented`: whether one was in
    /// force.
    pub async fn release_dolt(
        &self,
        presented: &MachineCertificate,
        origin: &Origin,
    ) -> Result<bool, DoltHoldError> {
        let dolt = self.dolt.clone().ok_or(DoltHoldError::Unavailable)?;
        let machine = self.app_machine(presented, origin).await?;
        let in_force = dolt.release();
        let action = if in_force {
            "dolt:released"
        } else {
            "dolt:expired"
        };
        self.audit_hold(DoltReleased, Some(&machine.name), action, origin)
            .await;
        Ok(in_force)
    }

    /// The application VM's machine of `presented`, or refused and audited.
    async fn app_machine(
        &self,
        presented: &MachineCertificate,
        origin: &Origin,
    ) -> Result<Machine, DoltHoldError> {
        let found = self
            .caller(presented, MachineRole::App)
            .await
            .map_err(|e| DoltHoldError::Backend(format!("{e:?}")))?;
        match found {
            Some(machine) => Ok(machine),
            None => {
                let name = presented.common_name.as_deref();
                self.audit_hold(CertificateRefused, name, "dolt:certificate", origin)
                    .await;
                Err(DoltHoldError::Refused)
            }
        }
    }
}
