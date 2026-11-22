//! Which machine calls a route that is not for renewing: known by the
//! certificate Traefik's internal entry point verified (0066, point 5), for
//! a route that serves one role (Trino's rules, its hold: point 9), and
//! whether a certificate is the one its machine uses (point 14).

use auth_gateway::app::machines::accept_certificate;
use auth_gateway::{Machine, MachineCertificate, MachineRole, MACHINE_ORGANIZATION};

use super::{audit, MachinesError, MachinesService, Origin};

/// Whether a machine's certificate is the one it uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Acceptance {
    /// The machine is not retired and the serial is its own. It may be
    /// `pending`: it has a certificate but must not be given jobs.
    Accepted(Box<Machine>),
    /// Unknown, retired, or a serial that is not the one in use.
    Refused,
}

impl MachinesService {
    /// The machine of `presented` if it is of `role` and may work: `O=qd`,
    /// its `OU` the role, accepted ([`Self::accepts`]: not retired, the
    /// serial in use or the next one, which becomes the one in use) and
    /// approved or draining, as for a token (point 6). `None` for anything
    /// else: a pending machine has a certificate but must not work yet.
    pub async fn caller(
        &self,
        presented: &MachineCertificate,
        role: MachineRole,
    ) -> Result<Option<Machine>, MachinesError> {
        if presented.organization.as_deref() != Some(MACHINE_ORGANIZATION)
            || presented.unit.as_deref() != Some(role.as_str())
        {
            return Ok(None);
        }
        let name = presented.common_name.as_deref().unwrap_or_default();
        Ok(match self.accepts(name, &presented.serial).await? {
            Acceptance::Accepted(machine)
                if machine.role == role && machine.state.gets_tokens() =>
            {
                Some(*machine)
            }
            _ => None,
        })
    }

    /// Record that `machine`, the update agent, asked for an update now for
    /// a person on the server (decision 0086).
    pub async fn update_requested(
        &self,
        machine: &Machine,
        origin: &Origin,
    ) -> Result<(), MachinesError> {
        let event = audit::update_requested(machine, origin, self.clock.now());
        Ok(self.record(event).await?)
    }

    /// Whether the certificate `serial` of `machine_name` is accepted: only
    /// when the machine is not retired and `serial` is the one it uses or
    /// its next one (0066, point 14). The next one, first presented, becomes
    /// the one in use, and the one before it is refused from then on, as a
    /// retired machine's is at once (point 9). A `pending` machine is
    /// accepted, as it has a certificate, but must not be given jobs: the
    /// caller tells pending from approved by the machine's state.
    /// The check is `auth_gateway`'s, shared with the token endpoint.
    pub async fn accepts(
        &self,
        machine_name: &str,
        serial: &str,
    ) -> Result<Acceptance, MachinesError> {
        let accepted = accept_certificate(self.registry.as_ref(), machine_name, serial).await?;
        Ok(match accepted {
            Some(machine) => Acceptance::Accepted(Box::new(machine)),
            None => Acceptance::Refused,
        })
    }
}
