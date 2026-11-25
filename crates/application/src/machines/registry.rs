//! A joined machine asks for the registry's pull token (quality-sdk
//! decision 0013): known by its certificate, which must be of an approved
//! machine that is not retired. The role is the certificate's; which
//! images it gets is the installed modules' ([`crate::UpdatesService::registry_grant`]).
//! Every call is audited as a machine call, never with the token.

use auth_gateway::{Machine, MachineCertificate, MachineState, MACHINE_ORGANIZATION};
use domain::AuditEventType::{CertificateRefused, CredentialsIssued};

use super::calling::Acceptance;
use super::{audit, MachinesError, MachinesService, Origin};

/// How a machine's call for the registry token ended, for the audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryOutcome {
    /// It got the token and its role's images.
    Granted,
    /// No installed module declares its role.
    NoSuchRole,
    /// No token held: it asks again later.
    NoToken,
}

impl MachinesService {
    /// The machine of `presented` when it is approved, not retired, and its
    /// certificate is the one in use and names its role; `None` otherwise,
    /// audited as a refusal.
    pub async fn registry_caller(
        &self,
        presented: &MachineCertificate,
        origin: &Origin,
    ) -> Result<Option<Machine>, MachinesError> {
        let name = presented.common_name.as_deref().unwrap_or_default();
        let found = if presented.organization.as_deref() == Some(MACHINE_ORGANIZATION) {
            match self.accepts(name, &presented.serial).await? {
                Acceptance::Accepted(machine)
                    if machine.state == MachineState::Approved
                        && presented.unit.as_deref() == Some(machine.role.as_str()) =>
                {
                    Some(*machine)
                }
                _ => None,
            }
        } else {
            None
        };
        if found.is_none() {
            let event = audit::machine_call(
                CertificateRefused,
                Some(name).filter(|n| !n.is_empty()),
                "registry:certificate".into(),
                origin,
                self.clock.now(),
            );
            if let Err(e) = self.record(event).await {
                tracing::warn!("registry: the refusal could not be audited: {e}");
            }
        }
        Ok(found)
    }

    /// Record how `machine`'s call for the registry token ended:
    /// `registry:<role>` (given), `registry:<role>:no-role` or
    /// `registry:<role>:no-token`, on `machine/<name>`.
    pub async fn registry_called(
        &self,
        machine: &Machine,
        outcome: RegistryOutcome,
        origin: &Origin,
    ) -> Result<(), MachinesError> {
        let role = machine.role.as_str();
        let (event_type, action) = match outcome {
            RegistryOutcome::Granted => (CredentialsIssued, format!("registry:{role}")),
            RegistryOutcome::NoSuchRole => (CertificateRefused, format!("registry:{role}:no-role")),
            RegistryOutcome::NoToken => (CertificateRefused, format!("registry:{role}:no-token")),
        };
        let event = audit::machine_call(
            event_type,
            Some(&machine.name),
            action,
            origin,
            self.clock.now(),
        );
        Ok(self.record(event).await?)
    }
}
