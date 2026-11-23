//! Authentication audit trail: the model and port for recording — and later
//! querying — every significant auth event.
//!
//! Following the same hexagonal discipline as the `auth-gateway` crate's
//! domain: pure value objects and a port ([`AuditRepository`]) the
//! `infrastructure` layer implements. No event ever carries a password or a
//! session/CSRF token — see [`NewAuditEvent`] for exactly what a record may
//! hold.

use std::time::SystemTime;

use crate::{AdminId, NewViewAudit, ViewShown};

/// An opaque identifier for a stored audit event.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AuditId(String);

impl AuditId {
    /// Wrap an identifier string.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AuditId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The kind of event recorded.
///
/// A closed set, matching exactly the events this codebase can actually emit
/// today — `login_succeeded`/`login_failed`/`locked_out` (from
/// `auth_gateway::LoginService`) and `logged_out` (from
/// `auth_gateway::SessionRepository::delete`'s caller). Refresh and
/// password changes join this set once those features exist; adding a variant
/// here is the only change their audit hook needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuditEventType {
    /// A login attempt succeeded.
    LoginSucceeded,
    /// A login attempt failed — wrong password or no such account
    /// (indistinguishable by design; see `auth_gateway::LoginError`).
    LoginFailed,
    /// A login attempt was rejected because the account or IP is locked out.
    LockedOut,
    /// A session was revoked via logout.
    LoggedOut,
    /// Someone asked for a password-reset email (whether or not the account
    /// exists — `admin_id` is set only when it does).
    PasswordResetRequested,
    /// A password was changed through a reset link.
    PasswordResetCompleted,
    /// An external sign-in identity (Google, Entra ID, ...) was linked to the
    /// account.
    OAuthLinked,
    /// An external sign-in identity was unlinked from the account.
    OAuthUnlinked,
    /// An admin created an account (`email_attempted` is the new account's).
    UserCreated,
    /// An admin sent an account its activation link again.
    ActivationSent,
    /// An admin deleted an account (`email_attempted` is the deleted one's).
    UserDeleted,
    /// An admin changed an account's name, role or area permissions
    /// (`email_attempted` is the changed one's).
    UserUpdated,
    /// Someone created a module's resource under an IAM action
    /// ([`NewActionAudit`]: `action`, `resource`).
    ResourceCreated,
    /// Someone changed a module's resource, not its secrets.
    ResourceChanged,
    /// Someone changed a resource's secrets: recorded as changed, never
    /// their value (decision 0026).
    SecretsChanged,
    /// Someone deleted a module's resource.
    ResourceDeleted,
    /// Someone started a run by hand (_Run now_, decision 0035).
    RunStarted,
    /// Someone approved a draft of the catalog (decision 0021).
    DraftApproved,
    /// A job was given credentials (a source's, or its warehouse's): for
    /// which job, to which machine (decision 0030, point 3).
    CredentialsIssued,
    /// Someone read a dataset's records with their values (a rule's test,
    /// decisions 0019 and 0043): who, which records' resource.
    RecordsViewed,
    /// Someone issued a token for a machine to join (decision 0029): who,
    /// for which role and pool. Never the token nor its hash.
    JoinTokenIssued,
    /// A machine joined with a token: the machine, its role.
    JoinTokenUsed,
    /// A join was refused (a token unknown, used or expired, or a name
    /// taken): the name it asked for, and why in `action`.
    JoinTokenRefused,
    /// Someone approved a pending machine (decision 0066, point 1).
    MachineApproved,
    /// Someone started draining a machine.
    MachineDraining,
    /// Someone retired a machine: its certificate is refused from then on.
    MachineRetired,
    /// A retired machine's sessions on the queue were ended, or could not
    /// be (decision 0066, point 4): how many, or `failed`, in `action`.
    MachineSessionsEnded,
    /// A machine was given a certificate (decision 0066): the machine, never
    /// the certificate.
    CertificateIssued,
    /// A machine's certificate was refused at renewal (unknown, replaced,
    /// retired, of another role) or its request was bad: why in `action`.
    CertificateRefused,
    /// Retired (decision 0077): never written again, kept so the rows
    /// already stored still read and their chain still hashes. Trino's
    /// coordinator asked whether it may restart (decision 0066, point 9):
    /// the jobs that use Trino held (`trino:held`), or not, as jobs or
    /// queries were running (`trino:busy`).
    TrinoHeld,
    /// Retired (decision 0077), as [`Self::TrinoHeld`]. The hold of Trino's
    /// jobs ended by its coordinator (`trino:released`), or found already
    /// expired by itself (`trino:expired`).
    TrinoReleased,
    /// The application VM's agent asked whether Dolt may restart (decision
    /// 0066, point 11): the catalog held (`dolt:held`), or not, as an
    /// operation or a statement was running (`dolt:busy`).
    DoltHeld,
    /// Dolt's hold ended by the agent (`dolt:released`), or found already
    /// expired by itself (`dolt:expired`).
    DoltReleased,
    /// The update agent's machine asked for an update now, for a person on
    /// the server (decision 0086): the machine, on `platform/updates`.
    UpdateRequested,
}

impl AuditEventType {
    /// The stable string form persisted to storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LoginSucceeded => "login_succeeded",
            Self::LoginFailed => "login_failed",
            Self::LockedOut => "locked_out",
            Self::LoggedOut => "logged_out",
            Self::PasswordResetRequested => "password_reset_requested",
            Self::PasswordResetCompleted => "password_reset_completed",
            Self::OAuthLinked => "oauth_linked",
            Self::OAuthUnlinked => "oauth_unlinked",
            Self::UserCreated => "user_created",
            Self::ActivationSent => "activation_sent",
            Self::UserDeleted => "user_deleted",
            Self::UserUpdated => "user_updated",
            Self::ResourceCreated => "resource_created",
            Self::ResourceChanged => "resource_changed",
            Self::SecretsChanged => "secrets_changed",
            Self::ResourceDeleted => "resource_deleted",
            Self::RunStarted => "run_started",
            Self::DraftApproved => "draft_approved",
            Self::CredentialsIssued => "credentials_issued",
            Self::RecordsViewed => "records_viewed",
            Self::JoinTokenIssued => "join_token_issued",
            Self::JoinTokenUsed => "join_token_used",
            Self::JoinTokenRefused => "join_token_refused",
            Self::MachineApproved => "machine_approved",
            Self::MachineDraining => "machine_draining",
            Self::MachineRetired => "machine_retired",
            Self::MachineSessionsEnded => "machine_sessions_ended",
            Self::CertificateIssued => "certificate_issued",
            Self::CertificateRefused => "certificate_refused",
            Self::TrinoHeld => "trino_held",
            Self::DoltHeld => "dolt_held",
            Self::DoltReleased => "dolt_released",
            Self::UpdateRequested => "update_requested",
            Self::TrinoReleased => "trino_released",
        }
    }

    /// Parse the string form written by [`Self::as_str`].
    pub fn parse(raw: &str) -> Result<Self, AuditEventTypeError> {
        Ok(match raw {
            "login_succeeded" => Self::LoginSucceeded,
            "login_failed" => Self::LoginFailed,
            "locked_out" => Self::LockedOut,
            "logged_out" => Self::LoggedOut,
            "password_reset_requested" => Self::PasswordResetRequested,
            "password_reset_completed" => Self::PasswordResetCompleted,
            "oauth_linked" => Self::OAuthLinked,
            "oauth_unlinked" => Self::OAuthUnlinked,
            "user_created" => Self::UserCreated,
            "activation_sent" => Self::ActivationSent,
            "user_deleted" => Self::UserDeleted,
            "user_updated" => Self::UserUpdated,
            "resource_created" => Self::ResourceCreated,
            "resource_changed" => Self::ResourceChanged,
            "secrets_changed" => Self::SecretsChanged,
            "resource_deleted" => Self::ResourceDeleted,
            "run_started" => Self::RunStarted,
            "draft_approved" => Self::DraftApproved,
            "credentials_issued" => Self::CredentialsIssued,
            "records_viewed" => Self::RecordsViewed,
            "join_token_issued" => Self::JoinTokenIssued,
            "join_token_used" => Self::JoinTokenUsed,
            "join_token_refused" => Self::JoinTokenRefused,
            "machine_approved" => Self::MachineApproved,
            "machine_draining" => Self::MachineDraining,
            "machine_retired" => Self::MachineRetired,
            "machine_sessions_ended" => Self::MachineSessionsEnded,
            "certificate_issued" => Self::CertificateIssued,
            "certificate_refused" => Self::CertificateRefused,
            "trino_held" => Self::TrinoHeld,
            "dolt_held" => Self::DoltHeld,
            "dolt_released" => Self::DoltReleased,
            "update_requested" => Self::UpdateRequested,
            "trino_released" => Self::TrinoReleased,
            other => return Err(AuditEventTypeError::Unknown(other.to_string())),
        })
    }
}

impl std::fmt::Display for AuditEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A stored audit event's type string did not match any known
/// [`AuditEventType`] — a data-integrity fault, since every write goes
/// through [`AuditEventType::as_str`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditEventTypeError {
    /// The raw value read back from storage.
    Unknown(String),
}

impl std::fmt::Display for AuditEventTypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown(raw) => write!(f, "unknown audit event type {raw:?}"),
        }
    }
}

impl std::error::Error for AuditEventTypeError {}

/// A stored audit event, as the repository hands it back for display.
#[derive(Debug, Clone)]
pub struct AuditEvent {
    /// The event's own id.
    pub id: AuditId,
    /// What happened.
    pub event_type: AuditEventType,
    /// The admin this event concerns, once one is known. `None` for a login
    /// failure against an email that matches no account.
    pub admin_id: Option<AdminId>,
    /// The email address submitted, for login events — kept even when it
    /// matched no account, since that is exactly the case `admin_id` cannot
    /// identify. Never a password.
    pub email_attempted: Option<String>,
    /// The client IP the request arrived from.
    pub ip: String,
    /// The client's `User-Agent` header, if it sent one.
    pub user_agent: Option<String>,
    /// When the event happened.
    pub occurred_at: SystemTime,
    /// The IAM action and the resource, for an event done under one.
    pub action: Option<String>,
    pub resource: Option<String>,
    /// The job, for credentials issued to one.
    pub job_id: Option<String>,
    /// The service account it came with, or the machine.
    pub service_account: Option<String>,
    /// A view's reason and shape (decision 0069); `None` before it.
    pub view: Option<ViewShown>,
}

/// A new audit event to persist.
///
/// Deliberately has no field for a password, a session token, or a CSRF
/// token — there is nowhere in this type to put one, which is what makes "no
/// sensitive data in the audit trail" a property of the type rather than a
/// convention callers must remember.
#[derive(Debug, Clone)]
pub struct NewAuditEvent {
    /// What happened.
    pub event_type: AuditEventType,
    /// The admin this event concerns, once one is known.
    pub admin_id: Option<AdminId>,
    /// The email address submitted, for login events.
    pub email_attempted: Option<String>,
    /// The client IP the request arrived from.
    pub ip: String,
    /// The client's `User-Agent` header, if it sent one.
    pub user_agent: Option<String>,
    /// When the event happened.
    pub occurred_at: SystemTime,
}

/// A new audit event of something done under an IAM action on a resource
/// (decision 0026: the audit log grows from administration to quality).
/// Like [`NewAuditEvent`], it has nowhere to put a value or a secret.
#[derive(Debug, Clone)]
pub struct NewActionAudit {
    pub event_type: AuditEventType,
    /// Who did it; of a service account, its owner. `None` for a machine.
    pub admin_id: Option<AdminId>,
    /// The service account it came with, or the machine (its client id).
    pub service_account: Option<String>,
    /// The job it was for, if any.
    pub job_id: Option<String>,
    /// The IAM action (`quality:ManageSources`); for credentials issued, the
    /// job's kind (`job:extract`).
    pub action: String,
    /// What it was done on (`source/core`).
    pub resource: String,
    pub ip: String,
    pub user_agent: Option<String>,
    pub occurred_at: SystemTime,
}

/// Port: persistence for the audit trail.
///
/// Implemented by a Postgres adapter in `infrastructure`. Recording is
/// best-effort from the caller's point of view — a delivery-layer caller logs
/// (rather than fails the request) if `record` itself errors, since an outage
/// in the audit store must never block a real login or logout.
#[async_trait::async_trait]
pub trait AuditRepository: Send + Sync {
    /// Persist a new audit event.
    async fn record(&self, event: &NewAuditEvent) -> Result<AuditId, crate::RepositoryError>;

    /// Persist an event of something done under an IAM action.
    async fn record_action(
        &self,
        event: &NewActionAudit,
    ) -> Result<AuditId, crate::RepositoryError>;

    /// Persist a view of values with its reason and shape (decision 0069).
    /// Refused unless the store says otherwise.
    async fn record_view(&self, _event: &NewViewAudit) -> Result<AuditId, crate::RepositoryError> {
        Err(crate::RepositoryError::Backend(
            "views are not recorded".into(),
        ))
    }

    /// The most recent events, newest first, capped at `limit`.
    async fn list_recent(&self, limit: u32) -> Result<Vec<AuditEvent>, crate::RepositoryError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_round_trips_through_its_string_form() {
        for event_type in [
            AuditEventType::LoginSucceeded,
            AuditEventType::LoginFailed,
            AuditEventType::LockedOut,
            AuditEventType::LoggedOut,
            AuditEventType::PasswordResetRequested,
            AuditEventType::PasswordResetCompleted,
            AuditEventType::OAuthLinked,
            AuditEventType::OAuthUnlinked,
            AuditEventType::UserCreated,
            AuditEventType::ActivationSent,
            AuditEventType::UserDeleted,
            AuditEventType::UserUpdated,
            AuditEventType::ResourceCreated,
            AuditEventType::ResourceChanged,
            AuditEventType::SecretsChanged,
            AuditEventType::ResourceDeleted,
            AuditEventType::RunStarted,
            AuditEventType::DraftApproved,
            AuditEventType::CredentialsIssued,
            AuditEventType::RecordsViewed,
            AuditEventType::JoinTokenIssued,
            AuditEventType::JoinTokenUsed,
            AuditEventType::JoinTokenRefused,
            AuditEventType::MachineApproved,
            AuditEventType::MachineDraining,
            AuditEventType::MachineRetired,
            AuditEventType::MachineSessionsEnded,
            AuditEventType::CertificateIssued,
            AuditEventType::CertificateRefused,
            AuditEventType::TrinoHeld,
            AuditEventType::TrinoReleased,
            AuditEventType::DoltHeld,
            AuditEventType::DoltReleased,
            AuditEventType::UpdateRequested,
        ] {
            assert_eq!(
                AuditEventType::parse(event_type.as_str()).unwrap(),
                event_type
            );
        }
    }

    #[test]
    fn the_retired_trino_events_still_parse() {
        // Stored rows of Trino's hold, which nothing writes since 0077.
        assert_eq!(
            AuditEventType::parse("trino_held").unwrap(),
            AuditEventType::TrinoHeld
        );
        assert_eq!(
            AuditEventType::parse("trino_released").unwrap(),
            AuditEventType::TrinoReleased
        );
    }

    #[test]
    fn unknown_event_type_string_is_rejected() {
        assert!(matches!(
            AuditEventType::parse("not_a_real_event"),
            Err(AuditEventTypeError::Unknown(_))
        ));
    }
}
