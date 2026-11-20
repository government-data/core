//! HTTP boundary for Settings → Users: list the accounts, create one
//! (it gets an activation link by email), send that link again, change its
//! name or role, delete one.
//! For whoever may do `admin:ManageUsers` (ADR 0016); only an `admin` gives
//! or takes the `admin` role.
//!
//! Holds no business logic — [`UsersService`] owns the rules; this module
//! only speaks HTTP and records the audit trail.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    AuditService, MailLoader, NewUser, SessionService, UserChanges, UsersError, UsersService,
    ASSIGNABLE_ROLES,
};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use domain::{AdminId, AuditEventType, NewAuditEvent};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::http::auth::ClientIp;
use crate::http::password_reset::ResetLinkBase;
use crate::http::rbac::require_action;
use crate::http::session::{require_session, CurrentSession};
use http_kit::error::ApiError;

#[derive(Clone)]
pub struct UsersState {
    users: UsersService,
    mail: Arc<dyn MailLoader>,
    audit: AuditService,
    links: ResetLinkBase,
}

impl std::fmt::Debug for UsersState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsersState").finish_non_exhaustive()
    }
}

/// Mount the user-management routes.
pub fn routes(
    users: UsersService,
    mail: Arc<dyn MailLoader>,
    sessions: SessionService,
    audit: AuditService,
    links: ResetLinkBase,
) -> Router {
    Router::new()
        .route("/users", get(list_handler).post(create_handler))
        .route("/users/{id}", delete(delete_handler).patch(update_handler))
        .route("/users/{id}/activation", post(resend_handler))
        .with_state(UsersState {
            users,
            mail: mail.clone(),
            audit,
            links,
        })
        // The current tenant's mail, loaded before sending the link.
        .layer(axum::middleware::from_fn_with_state(mail, loaded))
        .layer(axum::middleware::from_fn_with_state(
            iam::catalog::MANAGE_USERS,
            require_action,
        ))
        .layer(axum::middleware::from_fn_with_state(
            sessions,
            require_session,
        ))
}

async fn loaded(
    State(mail): State<Arc<dyn MailLoader>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if let Err(e) = mail.load_mail().await {
        return ApiError::internal(format!("users: settings: {e}")).into_response();
    }
    next.run(request).await
}

/// One account in the list.
#[derive(Debug, Serialize, ToSchema)]
pub struct UserOut {
    id: String,
    email: String,
    display_name: Option<String>,
    /// `admin` or `user` (or another role from the directory).
    role: String,
    /// `active` (has a password), `pending` (waiting for its activation) or
    /// `directory` (signs in with LDAP / AD).
    status: String,
    /// When it was created, in Unix epoch seconds.
    created_at: i64,
}

/// The accounts, plus what the creation form needs.
#[derive(Debug, Serialize, ToSchema)]
pub struct UsersOut {
    users: Vec<UserOut>,
    /// The roles a new account can get.
    roles: Vec<String>,
    /// Whether outgoing mail is configured (creating an account needs it).
    mail_configured: bool,
}

/// A new account.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateUserIn {
    #[schema(example = "ana@empresa.com")]
    email: String,
    display_name: Option<String>,
    /// `admin` or `user`.
    #[schema(example = "user")]
    role: String,
}

/// An account's new name and role.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateUserIn {
    /// Blank or absent clears it.
    display_name: Option<String>,
    /// `admin` or `user`.
    #[schema(example = "user")]
    role: String,
}

/// The created account.
#[derive(Debug, Serialize, ToSchema)]
pub struct CreatedUserOut {
    id: String,
    /// `false` when the account was created but the email could not be sent
    /// (send it again from the list).
    activation_sent: bool,
}

/// `GET /users`: every account.
#[utoipa::path(
    get,
    path = "/users",
    responses(
        (status = 200, description = "The accounts", body = UsersOut),
        (status = 401, description = "No valid session"),
        (status = 403, description = "Not an admin"),
    ),
    tag = "users",
)]
pub async fn list_handler(State(state): State<UsersState>) -> Response {
    match state.users.list().await {
        Ok(list) => Json(UsersOut {
            users: list
                .into_iter()
                .map(|a| UserOut {
                    id: a.id.as_str().to_string(),
                    email: a.email.as_str().to_string(),
                    display_name: a.display_name,
                    role: a.role.as_str().to_string(),
                    status: a.status.as_str().to_string(),
                    created_at: epoch(a.created_at),
                })
                .collect(),
            roles: state
                .users
                .roles()
                .await
                .unwrap_or_else(|_| ASSIGNABLE_ROLES.iter().map(|r| r.to_string()).collect()),
            mail_configured: mail_configured(&state).await,
        })
        .into_response(),
        Err(e) => error(e),
    }
}

/// `POST /users`: create an account and email it its activation link.
#[utoipa::path(
    post,
    path = "/users",
    request_body = CreateUserIn,
    responses(
        (status = 201, description = "Created", body = CreatedUserOut),
        (status = 403, description = "Not an admin, or missing CSRF header"),
        (status = 409, description = "Another account has that email"),
        (status = 422, description = "Invalid email or role"),
        (status = 503, description = "Outgoing mail is not configured"),
    ),
    tag = "users",
)]
pub async fn create_handler(
    State(state): State<UsersState>,
    Extension(current): Extension<CurrentSession>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(body): Json<CreateUserIn>,
) -> Response {
    let Some(base) = state.links.resolve(&headers) else {
        return ApiError::bad_request("no_host", "The request has no Host header.").into_response();
    };
    let is_admin = current.acts_as_admin();
    if let Err(e) = state
        .users
        .guard_admin(is_admin, None, Some(&body.role))
        .await
    {
        return error(e);
    }
    let email = body.email.trim().to_ascii_lowercase();
    let created = match state
        .users
        .create(
            NewUser {
                email: body.email,
                display_name: body.display_name,
                role: body.role,
            },
            &base,
        )
        .await
    {
        Ok(created) => created,
        Err(e) => return error(e),
    };
    state
        .record(
            AuditEventType::UserCreated,
            &current,
            Some(email),
            ip,
            &headers,
        )
        .await;
    (
        StatusCode::CREATED,
        Json(CreatedUserOut {
            id: created.id.as_str().to_string(),
            activation_sent: created.activation_sent,
        }),
    )
        .into_response()
}

/// `POST /users/{id}/activation`: email a pending account its activation
/// link again (the earlier one stops working).
#[utoipa::path(
    post,
    path = "/users/{id}/activation",
    params(("id" = String, Path, description = "The account id")),
    responses(
        (status = 204, description = "Sent"),
        (status = 404, description = "No such account"),
        (status = 409, description = "The account is already active"),
        (status = 502, description = "The email could not be sent"),
        (status = 503, description = "Outgoing mail is not configured"),
    ),
    tag = "users",
)]
pub async fn resend_handler(
    State(state): State<UsersState>,
    Extension(current): Extension<CurrentSession>,
    Path(id): Path<String>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
) -> Response {
    let Some(base) = state.links.resolve(&headers) else {
        return ApiError::bad_request("no_host", "The request has no Host header.").into_response();
    };
    match state.users.resend(&AdminId::new(id), &base).await {
        Ok(account) => {
            state
                .record(
                    AuditEventType::ActivationSent,
                    &current,
                    Some(account.email.as_str().to_string()),
                    ip,
                    &headers,
                )
                .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => error(e),
    }
}

/// `PATCH /users/{id}`: change an account's name and role (not one's own
/// role, not the last admin's, not a directory account's).
#[utoipa::path(
    patch,
    path = "/users/{id}",
    params(("id" = String, Path, description = "The account id")),
    request_body = UpdateUserIn,
    responses(
        (status = 204, description = "Changed"),
        (status = 404, description = "No such account"),
        (status = 409, description = "Your own role, the only admin, or a directory account"),
        (status = 422, description = "Invalid role or name"),
    ),
    tag = "users",
)]
pub async fn update_handler(
    State(state): State<UsersState>,
    Extension(current): Extension<CurrentSession>,
    Path(id): Path<String>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(body): Json<UpdateUserIn>,
) -> Response {
    let actor = AdminId::new(&current.admin_id);
    let target = AdminId::new(&id);
    let is_admin = current.acts_as_admin();
    if let Err(e) = state
        .users
        .guard_admin(is_admin, Some(&target), Some(&body.role))
        .await
    {
        return error(e);
    }
    let changes = UserChanges {
        display_name: body.display_name,
        role: body.role,
    };
    match state.users.update(&actor, &AdminId::new(id), changes).await {
        Ok(email) => {
            state
                .record(
                    AuditEventType::UserUpdated,
                    &current,
                    Some(email.as_str().to_string()),
                    ip,
                    &headers,
                )
                .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => error(e),
    }
}

/// `DELETE /users/{id}`: delete an account (not one's own, not the last
/// admin).
#[utoipa::path(
    delete,
    path = "/users/{id}",
    params(("id" = String, Path, description = "The account id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "No such account"),
        (status = 409, description = "It is your own account, or the only admin"),
    ),
    tag = "users",
)]
pub async fn delete_handler(
    State(state): State<UsersState>,
    Extension(current): Extension<CurrentSession>,
    Path(id): Path<String>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
) -> Response {
    let actor = AdminId::new(&current.admin_id);
    let target = AdminId::new(id);
    if let Err(e) = state
        .users
        .guard_admin(current.acts_as_admin(), Some(&target), None)
        .await
    {
        return error(e);
    }
    match state.users.delete(&actor, &target).await {
        Ok(email) => {
            state
                .record(
                    AuditEventType::UserDeleted,
                    &current,
                    Some(email.as_str().to_string()),
                    ip,
                    &headers,
                )
                .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => error(e),
    }
}

impl UsersState {
    /// Best-effort audit: the admin who did it, and the account it was done to.
    async fn record(
        &self,
        event_type: AuditEventType,
        current: &CurrentSession,
        target_email: Option<String>,
        ip: String,
        headers: &HeaderMap,
    ) {
        let user_agent = headers
            .get(axum::http::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        if let Err(e) = self
            .audit
            .record(NewAuditEvent {
                event_type,
                admin_id: Some(AdminId::new(&current.admin_id)),
                email_attempted: target_email,
                ip,
                user_agent,
                occurred_at: SystemTime::now(),
            })
            .await
        {
            tracing::warn!("users: failed to record audit event: {e}");
        }
    }
}

/// Whether the current tenant has outgoing mail (already loaded by `loaded`).
async fn mail_configured(state: &UsersState) -> bool {
    state.mail.load_mail().await.is_ok_and(|m| m.configured())
}

fn epoch(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn error(e: UsersError) -> Response {
    match e {
        UsersError::Invalid(field, reason) => {
            let mut fields = BTreeMap::new();
            fields.insert(field.to_string(), reason);
            ApiError::validation(fields).into_response()
        }
        UsersError::EmailTaken => {
            ApiError::conflict("email_taken", "Another account already has that email.")
                .into_response()
        }
        UsersError::NotFound => ApiError::not_found("No such account.").into_response(),
        UsersError::NotPending => {
            ApiError::conflict("already_active", "The account is already active.").into_response()
        }
        UsersError::SelfDelete => {
            ApiError::conflict("self_delete", "You cannot delete your own account.").into_response()
        }
        UsersError::LastAdmin => {
            ApiError::conflict("last_admin", "It is the only admin; it cannot be deleted.")
                .into_response()
        }
        UsersError::SelfRole => {
            ApiError::conflict("self_role", "You cannot change your own role.").into_response()
        }
        UsersError::AdminOnly => {
            ApiError::forbidden("admin_only", "Only an admin can do that to an admin.")
                .into_response()
        }
        UsersError::DirectoryManaged => ApiError::conflict(
            "directory_managed",
            "The directory sets that account's name and role.",
        )
        .into_response(),
        UsersError::MailUnavailable => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "mail_not_configured",
            "Activation links need outgoing mail, which is not configured.",
        )
        .into_response(),
        UsersError::MailFailed(e) => {
            ApiError::bad_gateway("mail_failed", "The activation email could not be sent.")
                .with_internal(e)
                .into_response()
        }
        UsersError::Internal(e) => ApiError::internal(format!("users: {e}")).into_response(),
    }
}
