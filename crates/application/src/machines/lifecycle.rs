//! A machine's states (0029, 0066 point 4): pending → approved → draining
//! → retired, each change audited, retiring ending its sessions on the
//! queue; and the serial of the certificate it uses.

use std::time::SystemTime;

use auth_gateway::{Machine, MachineChange, MachineId, MachineState};
use domain::AdminId;
use domain::AuditEventType::{self, MachineApproved, MachineDraining, MachineRetired};
use MachineState::{Approved, Draining, Retired};

use super::{audit, MachinesError, MachinesService, Origin};

impl MachinesService {
    /// A person lets a pending machine take work.
    pub async fn approve(
        &self,
        machine: &MachineId,
        by: &AdminId,
        origin: &Origin,
    ) -> Result<Machine, MachinesError> {
        self.transition(machine, Approved, MachineApproved, by, origin)
            .await
    }

    /// An approved machine finishes what it has and gets nothing new.
    pub async fn drain(
        &self,
        machine: &MachineId,
        by: &AdminId,
        origin: &Origin,
    ) -> Result<Machine, MachinesError> {
        self.transition(machine, Draining, MachineDraining, by, origin)
            .await
    }

    /// A machine is out for good: its certificate is refused from now on.
    pub async fn retire(
        &self,
        machine: &MachineId,
        by: &AdminId,
        origin: &Origin,
    ) -> Result<Machine, MachinesError> {
        let machine = self
            .transition(machine, Retired, MachineRetired, by, origin)
            .await?;
        self.end_sessions(&machine, by, origin).await;
        Ok(machine)
    }

    /// Ends a retired machine's sessions on the queue (0066, point 4), after
    /// the retirement is stored: best effort, as nothing here undoes it. A
    /// failure is logged and audited; so is what was ended.
    async fn end_sessions(&self, machine: &Machine, by: &AdminId, origin: &Origin) {
        let Some(sessions) = &self.sessions else {
            return;
        };
        let ended = match sessions.end(&machine.name).await {
            Ok(n) => Some(n),
            Err(e) => {
                tracing::warn!(machine = %machine.name, error = %e,
                    "the retired machine's queue sessions could not be ended");
                None
            }
        };
        let event = audit::sessions_ended(machine, ended, by, origin, self.clock.now());
        if let Err(e) = self.record(event).await {
            tracing::warn!(machine = %machine.name, error = %e,
                "the end of a retired machine's sessions could not be audited");
        }
    }

    async fn transition(
        &self,
        machine: &MachineId,
        to: MachineState,
        event: AuditEventType,
        by: &AdminId,
        origin: &Origin,
    ) -> Result<Machine, MachinesError> {
        let now = self.clock.now();
        let machine = done(self.registry.transition(machine, to, by, now).await?)?;
        self.record(audit::transition(&machine, event, by, origin, now))
            .await?;
        Ok(machine)
    }

    /// The certificate `serial` (hex) is now the one `machine` uses, until
    /// `not_after`; any other it had is refused from now on. Refused for a
    /// retired machine. Audited before the certificate is handed over: if
    /// this fails, the caller does not give it.
    pub async fn record_certificate(
        &self,
        machine: &MachineId,
        serial: &str,
        not_after: SystemTime,
        origin: &Origin,
    ) -> Result<Machine, MachinesError> {
        let serial = serial.to_ascii_lowercase();
        if serial.is_empty() || serial.len() > 64 || !serial.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(MachinesError::Invalid("serial"));
        }
        let change = self
            .registry
            .record_certificate(machine, &serial, not_after)
            .await?;
        let machine = done(change)?;
        self.record(audit::certificate_issued(
            &machine,
            origin,
            self.clock.now(),
        ))
        .await?;
        Ok(machine)
    }
}

pub(super) fn done(change: MachineChange) -> Result<Machine, MachinesError> {
    match change {
        MachineChange::Done(machine) => Ok(*machine),
        MachineChange::NotAllowed(state) => Err(MachinesError::NotAllowed(state)),
        MachineChange::NotFound => Err(MachinesError::NotFound),
    }
}
