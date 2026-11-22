//! `POST /internal/admin-access`: the link for the administrator to choose
//! their password. The installer asks for it when it finishes (it creates the
//! account if there is none) and the host's admin access command when the
//! password was lost. Whoever gets here is root on the server.

use std::time::SystemTime;

use auth_gateway::{AccessError, AuditService, PasswordResetService};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use domain::{AuditEventType, NewAuditEvent};
use serde::Deserialize;
use serde_json::json;

use super::{unavailable, Internal};

/// The user agent the audit records for a link issued from the host.
const HOST_USER_AGENT: &str = "host (internal routes)";

/// What `admin-access` needs.
#[derive(Debug, Clone)]
pub struct AdminAccessRoute {
    pub service: PasswordResetService,
    pub audit: AuditService,
    /// The web's public address, the one links use.
    pub link_base: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdminAccessBody {
    email: String,
    /// Also for an account that already chose its password (it lost it).
    #[serde(default)]
    force: bool,
}

pub(super) async fn admin_access(
    State(s): State<Internal>,
    Json(body): Json<AdminAccessBody>,
) -> Response {
    let Some(access) = s.access else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let result = access
        .service
        .admin_access(&body.email, &access.link_base, body.force)
        .await;
    let (status, error) = match &result {
        Ok(_) => (StatusCode::OK, String::new()),
        Err(AccessError::InvalidEmail(_)) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            texts::tr!("api.internal.invalidEmail"),
        ),
        Err(AccessError::UnknownAccount) => (
            StatusCode::NOT_FOUND,
            texts::tr!("api.internal.unknownAccount"),
        ),
        Err(AccessError::DirectoryAccount) => (
            StatusCode::CONFLICT,
            texts::tr!("api.internal.directoryAccount"),
        ),
        Err(AccessError::Internal(e)) => return unavailable(e),
    };
    let Ok(granted) = result else {
        return (status, Json(json!({ "error": error }))).into_response();
    };
    if granted.created || granted.link.is_some() {
        let event = NewAuditEvent {
            event_type: if granted.created {
                AuditEventType::UserCreated
            } else {
                AuditEventType::ActivationSent
            },
            admin_id: Some(granted.admin.clone()),
            email_attempted: Some(body.email.trim().to_ascii_lowercase()),
            ip: "127.0.0.1".into(),
            user_agent: Some(HOST_USER_AGENT.into()),
            occurred_at: SystemTime::now(),
        };
        if let Err(e) = access.audit.record(event).await {
            tracing::warn!("admin access: failed to record audit event: {e}");
        }
        tracing::warn!(
            created = granted.created,
            emailed = granted.emailed,
            "admin access link issued from the host"
        );
    }
    Json(json!({
        "created": granted.created,
        "link": granted.link,
        "emailed": granted.emailed,
    }))
    .into_response()
}
