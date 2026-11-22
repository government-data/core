//! What the machines' use cases write to the audit log. A machine appears as
//! the service account (its name) and as the resource `machine/<name>`;
//! `action` says the role, or why a join was refused. No token, no hash and
//! no certificate goes in.

use std::time::SystemTime;

use auth_gateway::domain::machine::is_label;
use auth_gateway::{Lake, Machine, MachineRole};
use domain::{AdminId, AuditEventType, NewActionAudit};

/// Where a request came from, for the audit log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub ip: String,
    pub user_agent: Option<String>,
}

fn event(
    event_type: AuditEventType,
    admin_id: Option<&AdminId>,
    machine: Option<&str>,
    action: String,
    resource: String,
    origin: &Origin,
    at: SystemTime,
) -> NewActionAudit {
    NewActionAudit {
        event_type,
        admin_id: admin_id.cloned(),
        service_account: machine.map(str::to_string),
        job_id: None,
        action,
        resource,
        ip: origin.ip.clone(),
        user_agent: origin.user_agent.clone(),
        occurred_at: at,
    }
}

fn resource(machine: &str) -> String {
    format!("machine/{machine}")
}

/// `join:<role>`, `join:<role>/<pool>`, or `join:<role>/<pool>/<node>`.
pub(super) fn token_issued(
    id: &str,
    role: MachineRole,
    pool: Option<&str>,
    node: Option<&str>,
    by: &AdminId,
    origin: &Origin,
    at: SystemTime,
) -> NewActionAudit {
    let action = match (pool, node) {
        (Some(pool), Some(node)) => format!("join:{}/{pool}/{node}", role.as_str()),
        (Some(pool), None) => format!("join:{}/{pool}", role.as_str()),
        (None, _) => format!("join:{}", role.as_str()),
    };
    let resource = format!("join-token/{id}");
    event(
        AuditEventType::JoinTokenIssued,
        Some(by),
        None,
        action,
        resource,
        origin,
        at,
    )
}

pub(super) fn join_used(machine: &Machine, origin: &Origin, at: SystemTime) -> NewActionAudit {
    event(
        AuditEventType::JoinTokenUsed,
        None,
        Some(&machine.name),
        format!("join:{}", machine.role.as_str()),
        resource(&machine.name),
        origin,
        at,
    )
}

/// `join:<reason>`: `token` (unknown, used or expired), `name-taken`,
/// `node-name` (not the node a pool's token names), `invalid-name` or
/// `invalid-request` (the signing request). A name out of form is not
/// written.
pub(super) fn join_refused(
    name: &str,
    reason: &str,
    origin: &Origin,
    at: SystemTime,
) -> NewActionAudit {
    let name = is_label(name).then_some(name);
    event(
        AuditEventType::JoinTokenRefused,
        None,
        name,
        format!("join:{reason}"),
        resource(name.unwrap_or("-")),
        origin,
        at,
    )
}

/// Approved, draining or retired, by whom.
pub(super) fn transition(
    machine: &Machine,
    event_type: AuditEventType,
    by: &AdminId,
    origin: &Origin,
    at: SystemTime,
) -> NewActionAudit {
    event(
        event_type,
        Some(by),
        Some(&machine.name),
        format!("machine:{}", machine.role.as_str()),
        resource(&machine.name),
        origin,
        at,
    )
}

/// A retired machine's queue sessions: `sessions:<how many>` ended, or
/// `sessions:failed`.
pub(super) fn sessions_ended(
    machine: &Machine,
    ended: Option<u64>,
    by: &AdminId,
    origin: &Origin,
    at: SystemTime,
) -> NewActionAudit {
    let action = match ended {
        Some(n) => format!("sessions:{n}"),
        None => "sessions:failed".to_string(),
    };
    event(
        AuditEventType::MachineSessionsEnded,
        Some(by),
        Some(&machine.name),
        action,
        resource(&machine.name),
        origin,
        at,
    )
}

pub(super) fn certificate_issued(
    machine: &Machine,
    origin: &Origin,
    at: SystemTime,
) -> NewActionAudit {
    event(
        AuditEventType::CertificateIssued,
        None,
        Some(&machine.name),
        format!("machine:{}", machine.role.as_str()),
        resource(&machine.name),
        origin,
        at,
    )
}

/// The installer made the machine approved, or signed its certificate on
/// the spot (0066, point 16): `install:<role>`, by no person.
pub(super) fn installed(
    machine: &Machine,
    event_type: AuditEventType,
    origin: &Origin,
    at: SystemTime,
) -> NewActionAudit {
    event(
        event_type,
        None,
        Some(&machine.name),
        format!("install:{}", machine.role.as_str()),
        resource(&machine.name),
        origin,
        at,
    )
}

/// The update agent's machine asked for an update now, for a person on the
/// server (decision 0086): `request` on `platform/updates`, by no person.
pub(super) fn update_requested(
    machine: &Machine,
    origin: &Origin,
    at: SystemTime,
) -> NewActionAudit {
    event(
        AuditEventType::UpdateRequested,
        None,
        Some(&machine.name),
        "request".into(),
        "platform/updates".into(),
        origin,
        at,
    )
}

/// A lake's root keys given to a node of `role` that is installing, before
/// it joins (0066 step 11): `install:<role>` on `lake/<lake>`, by no person
/// and no machine yet. Never the keys.
pub(super) fn lake_keys_given(
    lake: &Lake,
    role: MachineRole,
    origin: &Origin,
    at: SystemTime,
) -> NewActionAudit {
    event(
        AuditEventType::CredentialsIssued,
        None,
        None,
        format!("install:{}", role.as_str()),
        format!("lake/{}", lake.key()),
        origin,
        at,
    )
}

/// A lake's admin keys pasted by a person: `lake:admin-keys` on
/// `lake/<lake>`, never the keys.
pub(super) fn lake_admin_keys_set(
    lake: &Lake,
    by: &AdminId,
    origin: &Origin,
    at: SystemTime,
) -> NewActionAudit {
    event(
        AuditEventType::SecretsChanged,
        Some(by),
        None,
        "lake:admin-keys".into(),
        format!("lake/{}", lake.key()),
        origin,
        at,
    )
}

/// A renewed certificate, the machine's next one: `renew:<role>`.
pub(super) fn certificate_renewed(
    machine: &Machine,
    origin: &Origin,
    at: SystemTime,
) -> NewActionAudit {
    event(
        AuditEventType::CertificateIssued,
        None,
        Some(&machine.name),
        format!("renew:{}", machine.role.as_str()),
        resource(&machine.name),
        origin,
        at,
    )
}

/// Trino's hold (0066, point 9): `trino_held` or `trino_released` with
/// what happened, or `certificate_refused` with `trino:certificate`; and
/// Dolt's (point 11), `dolt_held`, `dolt_released`, `dolt:certificate`. A
/// name out of form is not written.
pub(super) fn hold(
    event_type: AuditEventType,
    machine: Option<&str>,
    action: &str,
    origin: &Origin,
    at: SystemTime,
) -> NewActionAudit {
    let name = machine.filter(|n| is_label(n));
    event(
        event_type,
        None,
        name,
        action.to_string(),
        resource(name.unwrap_or("-")),
        origin,
        at,
    )
}

/// `renew:<reason>`: `no-certificate`, `certificate` (unknown, replaced or
/// retired), `role` or `invalid-request`. A name out of form is not
/// written; the certificate never is.
pub(super) fn renew_refused(
    name: Option<&str>,
    reason: &str,
    origin: &Origin,
    at: SystemTime,
) -> NewActionAudit {
    let name = name.filter(|n| is_label(n));
    event(
        AuditEventType::CertificateRefused,
        None,
        name,
        format!("renew:{reason}"),
        resource(name.unwrap_or("-")),
        origin,
        at,
    )
}
