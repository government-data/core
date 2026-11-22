//! HTTP boundary for the audit trail: a read-only, admin-only endpoint the
//! admin panel's audit view (bead authapp-c418dc) queries.
//!
//! Holds no business logic — [`AuditService`] owns the query; this module
//! only translates it to/from HTTP.

use std::time::SystemTime;

use auth_gateway::{AuditService, SessionService};
use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use domain::AuditEvent;
use serde::{Deserialize, Serialize};

use crate::error::{ApiError, ErrorResponse};
use crate::rbac::require_action;
use crate::session::require_session;

/// Mount the audit routes, gated by [`require_session`] (any authenticated
/// session) *and* [`require_action`] the `admin:ViewAudit` action (ADR 0016;
/// `admin` has every capability). `GET` is exempt from CSRF (it is not a mutation),
/// so no CSRF header is required here.
///
/// Layer order matters: `require_action` is added first (so it becomes the inner
/// layer, running second) and `require_session` last (the outer layer,
/// running first) — `require_action` reads the `CurrentSession` only
/// `require_session` has populated by the time it runs.
pub fn routes(audit: AuditService, sessions: SessionService) -> Router {
    Router::new()
        .route("/audit/events", get(list_events))
        .with_state(audit)
        .layer(axum::middleware::from_fn_with_state(
            iam::catalog::VIEW_AUDIT,
            require_action,
        ))
        .layer(axum::middleware::from_fn_with_state(
            sessions,
            require_session,
        ))
}

/// `?limit=` query parameter, capped well below anything that could turn one
/// request into an unbounded table scan.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ListQuery {
    /// Maximum number of events to return (default 50, capped at 200).
    limit: Option<u32>,
}

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 200;

/// One audit event, as returned to the admin panel.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EventOut {
    /// The event's opaque id.
    id: String,
    /// The event type (`login_succeeded`, `login_failed`, `locked_out`,
    /// `logged_out`, `password_reset_requested`, `password_reset_completed`,
    /// `oauth_linked`, `oauth_unlinked`, `user_created`, `activation_sent`,
    /// `user_deleted`, `user_updated`, `resource_created`, `resource_changed`,
    /// `secrets_changed`, `resource_deleted`, `run_started`, `draft_approved`, `credentials_issued`,
    /// `records_viewed`, `join_token_issued`, `join_token_used`,
    /// `join_token_refused`, `machine_approved`, `machine_draining`,
    /// `machine_retired`, `machine_sessions_ended`, `certificate_issued`,
    /// `certificate_refused`, `trino_held`, `trino_released`, `dolt_held`,
    /// `dolt_released`, `update_requested`).
    event_type: String,
    /// The resolved admin id, when the event maps to a known account.
    admin_id: Option<String>,
    /// The email submitted, for login events (kept even when it matched no
    /// account).
    email_attempted: Option<String>,
    /// The client IP the request came from.
    ip: String,
    /// The client `User-Agent`, if any.
    user_agent: Option<String>,
    /// When the event happened, as Unix epoch seconds.
    occurred_at_epoch: u64,
    /// The IAM action it was done under, for an event on a module's resource.
    action: Option<String>,
    /// The resource it was done on (`source/core`).
    resource: Option<String>,
    /// The job, for credentials issued to one (the machine is in
    /// `service_account`), and for a view of values its view's job.
    job_id: Option<String>,
    /// The service account it came with, or the machine.
    service_account: Option<String>,
    /// For a view of values, why it was revealed: a purpose's key
    /// (`quality_review`, `rule_writing`, `remediation`, `client_request`,
    /// `audit`, `other`, or one no longer offered). Absent before decision
    /// 0069 and for every other event.
    purpose: Option<String>,
    /// For a view of values, the ticket or case number it was revealed
    /// under, if given.
    reference: Option<String>,
    /// For a view of values, how many rows it showed.
    rows_shown: Option<u32>,
    /// For a view of values, the names of the columns or fields it showed,
    /// in order (a console column that is not a plain identifier by its
    /// position, `#2`). Never a value.
    fields_shown: Option<Vec<String>>,
}

impl From<AuditEvent> for EventOut {
    fn from(e: AuditEvent) -> Self {
        let view = e.view;
        Self {
            id: e.id.as_str().to_string(),
            event_type: e.event_type.as_str().to_string(),
            admin_id: e.admin_id.map(|id| id.as_str().to_string()),
            email_attempted: e.email_attempted,
            ip: e.ip,
            user_agent: e.user_agent,
            occurred_at_epoch: e
                .occurred_at
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            action: e.action,
            resource: e.resource,
            job_id: e.job_id,
            service_account: e.service_account,
            purpose: view.as_ref().map(|v| v.purpose.clone()),
            reference: view.as_ref().and_then(|v| v.reference.clone()),
            rows_shown: view.as_ref().map(|v| v.rows_shown),
            fields_shown: view.map(|v| v.fields_shown),
        }
    }
}

/// Handle `GET /audit/events?limit=`: the most recent audit events, newest
/// first, capped at [`MAX_LIMIT`] regardless of what the caller asks for.
#[utoipa::path(
    get,
    path = "/audit/events",
    params(ListQuery),
    responses(
        (status = 200, description = "Recent audit events, newest first", body = [EventOut]),
        (status = 401, description = "No valid session", body = ErrorResponse),
        (status = 403, description = "Authenticated but not the admin role", body = ErrorResponse),
        (status = 500, description = "Internal error", body = ErrorResponse),
    ),
    tag = "audit",
)]
pub(crate) async fn list_events(
    State(audit): State<AuditService>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<EventOut>>, ApiError> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let events = audit
        .recent(limit)
        .await
        .map_err(|e| ApiError::internal(format!("audit: failed to list events: {e}")))?;
    Ok(Json(events.into_iter().map(EventOut::from).collect()))
}
