//! Whether a machine's certificate is the one it uses (decision 0066,
//! points 9 and 14): the one check both the machines' use cases
//! (`application::machines`, renewing) and the token endpoint (a machine's
//! token, point 6) make, so that they cannot drift apart.

use crate::domain::machine::is_label;
use crate::domain::{Machine, MachineChange, MachineRegistry, MachineState, RepositoryError};

/// The machine named `machine_name`, if the certificate `serial` (hex) is
/// accepted for it: only when it is not retired and `serial` is the one it
/// uses or its next one. The next one, first presented, becomes the one in
/// use, and the one before it is refused from then on, as a retired
/// machine's is at once. A `pending` machine is accepted, as it has a
/// certificate; what it may do is the caller's to decide by its state.
pub async fn accept_certificate(
    registry: &dyn MachineRegistry,
    machine_name: &str,
    serial: &str,
) -> Result<Option<Machine>, RepositoryError> {
    if !is_label(machine_name) {
        return Ok(None);
    }
    let Some(machine) = registry.find_live(machine_name).await? else {
        return Ok(None);
    };
    let is = |s: &Option<String>| s.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(serial));
    if machine.state == MachineState::Retired {
        return Ok(None);
    }
    if is(&machine.cert_serial) {
        return Ok(Some(machine));
    }
    let Some(next) = machine
        .cert_serial_next
        .clone()
        .filter(|_| is(&machine.cert_serial_next))
    else {
        return Ok(None);
    };
    Ok(
        match registry
            .promote_next_certificate(&machine.id, &next)
            .await?
        {
            MachineChange::Done(machine) => Some(*machine),
            _ => None,
        },
    )
}
