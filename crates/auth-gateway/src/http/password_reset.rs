//! HTTP boundary for password recovery.
//!
//! - `POST /auth/password-reset/request` — email a reset link. `202` whether
//!   or not the account exists, so the answer never confirms one.
//! - `POST /auth/password-reset/check` — whether a link still works, plus the
//!   password policy the form shows.
//! - `POST /auth/password-reset/confirm` — set the new password with the link.
//!
//! The token travels in request bodies, never in a URL, so it does not end up
//! in access logs. All three are public (no session) and rate-limited per IP;
//! the request also per submitted email, so nobody can flood a mailbox.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use crate::AuditService;
use crate::PasswordPolicy;
use crate::{PasswordResetService, ResetError};
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use domain::{AuditEventType, Email, NewAuditEvent};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::http::auth::ClientIp;
use http_kit::error::{ApiError, ErrorResponse};
use http_kit::rate_limit::{RateLimitConfig, RateLimiter};

/// Where the links in the emails point.
#[derive(Debug, Clone)]
pub enum ResetLinkBase {
    /// The fixed public address of the web app.
    Fixed(String),
}

impl ResetLinkBase {
    /// The base address of the links.
    pub(crate) fn resolve(&self) -> String {
        match self {
            Self::Fixed(base) => base.clone(),
        }
    }
}

#[derive(Clone)]
pub struct ResetState {
    service: PasswordResetService,
    audit: AuditService,
    links: ResetLinkBase,
    rate_limit: Arc<RateLimiter>,
}

impl std::fmt::Debug for ResetState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResetState").finish_non_exhaustive()
    }
}

/// Mount the password-recovery routes.
pub fn routes(
    service: PasswordResetService,
    audit: AuditService,
    links: ResetLinkBase,
    rate_limit: RateLimitConfig,
) -> Router {
    Router::new()
        .route("/auth/password-reset/request", post(request_handler))
        .route("/auth/password-reset/check", post(check_handler))
        .route("/auth/password-reset/confirm", post(confirm_handler))
        .with_state(ResetState {
            service,
            audit,
            links,
            rate_limit: Arc::new(RateLimiter::new(rate_limit)),
        })
}

/// Ask for a reset link.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ResetRequestBody {
    /// The account's email address.
    #[schema(example = "admin@example.com")]
    email: String,
    /// The Turnstile token (required when the API uses a captcha).
    #[serde(default)]
    captcha: Option<String>,
}

/// The answer to a reset request, the same for every email.
#[derive(Debug, Serialize, ToSchema)]
pub struct ResetRequestOut {
    /// How long the emailed link works, in minutes.
    expires_in_minutes: u64,
}

/// A reset link's token.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ResetCheckBody {
    /// The token from the link (the part after `#`).
    token: String,
}

/// The password rules a new password must meet.
#[derive(Debug, Serialize, ToSchema)]
pub struct PasswordPolicyOut {
    min_length: usize,
    require_uppercase: bool,
    require_lowercase: bool,
    require_digit: bool,
    require_symbol: bool,
}

impl From<PasswordPolicy> for PasswordPolicyOut {
    fn from(p: PasswordPolicy) -> Self {
        Self {
            min_length: p.min_length,
            require_uppercase: p.require_uppercase,
            require_lowercase: p.require_lowercase,
            require_digit: p.require_digit,
            require_symbol: p.require_symbol,
        }
    }
}

/// Whether a link still works.
#[derive(Debug, Serialize, ToSchema)]
pub struct ResetCheckOut {
    /// `false` for a link that is unknown, used, replaced or expired.
    valid: bool,
    policy: PasswordPolicyOut,
}

/// Set a new password with a reset link.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ResetConfirmBody {
    /// The token from the link (the part after `#`).
    token: String,
    /// The new password.
    password: String,
}

fn too_many_requests(retry_after_secs: u64) -> Response {
    let mut response = ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "too_many_attempts",
        "Too many attempts. Please try again later.",
    )
    .into_response();
    if let Ok(value) = HeaderValue::from_str(&retry_after_secs.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

impl ResetState {
    /// The `429` to answer with when any of `keys` is over its limit.
    fn limit(&self, keys: &[String]) -> Option<Response> {
        let now = SystemTime::now();
        keys.iter().find_map(|key| {
            let exceeded = self.rate_limit.check(key, now).err()?;
            tracing::warn!(key, "password reset rate limit exceeded");
            Some(too_many_requests(exceeded.retry_after.as_secs()))
        })
    }

    fn link_base(&self) -> String {
        self.links.resolve()
    }

    async fn audit(
        &self,
        event_type: AuditEventType,
        admin_id: Option<domain::AdminId>,
        email: Option<String>,
        ip: &str,
        headers: &HeaderMap,
    ) {
        let event = NewAuditEvent {
            event_type,
            admin_id,
            email_attempted: email,
            ip: ip.to_string(),
            user_agent: headers
                .get(header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string),
            occurred_at: SystemTime::now(),
        };
        if let Err(e) = self.audit.record(event).await {
            tracing::warn!("password reset: failed to record audit event: {e}");
        }
    }
}

fn mail_unavailable() -> Response {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "mail_not_configured",
        "Password recovery needs outgoing mail, which is not configured. Ask an administrator.",
    )
    .into_response()
}

fn invalid_token() -> Response {
    ApiError::new(
        StatusCode::GONE,
        "invalid_token",
        "This link is invalid or has expired. Ask for a new one.",
    )
    .into_response()
}

/// Handle `POST /auth/password-reset/request`.
#[utoipa::path(
    post,
    path = "/auth/password-reset/request",
    request_body = ResetRequestBody,
    responses(
        (status = 202, description = "If the account exists, a link is on its way", body = ResetRequestOut),
        (status = 422, description = "Malformed email", body = ErrorResponse),
        (status = 429, description = "Rate-limited; carries Retry-After", body = ErrorResponse),
        (status = 503, description = "Outgoing mail is not configured", body = ErrorResponse),
    ),
    tag = "auth",
)]
pub async fn request_handler(
    State(state): State<ResetState>,
    captcha: Option<axum::Extension<crate::http::captcha::Captcha>>,
    ClientIp(client_ip): ClientIp,
    headers: HeaderMap,
    Json(body): Json<ResetRequestBody>,
) -> Response {
    let Ok(email) = Email::parse(&body.email) else {
        return ApiError::validation(BTreeMap::from([(
            "email".to_string(),
            "must be a valid email address".to_string(),
        )]))
        .into_response();
    };
    if let Some(response) = state.limit(&[
        format!("ip:{client_ip}"),
        format!("acct:{}", email.as_str()),
    ]) {
        return response;
    }
    // Every request sends an email: with captcha, if there is one.
    if let Some(axum::Extension(c)) = &captcha {
        if let Err(e) = c.verify(body.captcha.as_deref(), &client_ip).await {
            return e.into_response();
        }
    }
    let base = state.link_base();
    match state.service.request(email.as_str(), &base).await {
        Ok(admin) => {
            state
                .audit(
                    AuditEventType::PasswordResetRequested,
                    admin,
                    Some(email.as_str().to_string()),
                    &client_ip,
                    &headers,
                )
                .await;
            (
                StatusCode::ACCEPTED,
                Json(ResetRequestOut {
                    expires_in_minutes: (state.service.ttl().as_secs() / 60).max(1),
                }),
            )
                .into_response()
        }
        Err(ResetError::MailUnavailable) => mail_unavailable(),
        Err(e) => ApiError::internal(format!("password reset request: {e}")).into_response(),
    }
}

/// Handle `POST /auth/password-reset/check`.
#[utoipa::path(
    post,
    path = "/auth/password-reset/check",
    request_body = ResetCheckBody,
    responses(
        (status = 200, description = "Whether the link works, and the password policy", body = ResetCheckOut),
        (status = 429, description = "Rate-limited; carries Retry-After", body = ErrorResponse),
    ),
    tag = "auth",
)]
pub async fn check_handler(
    State(state): State<ResetState>,
    ClientIp(client_ip): ClientIp,
    Json(body): Json<ResetCheckBody>,
) -> Response {
    if let Some(response) = state.limit(&[format!("ip:{client_ip}")]) {
        return response;
    }
    match state.service.check(&body.token).await {
        Ok(valid) => Json(ResetCheckOut {
            valid,
            policy: state.service.policy().into(),
        })
        .into_response(),
        Err(e) => ApiError::internal(format!("password reset check: {e}")).into_response(),
    }
}

/// Handle `POST /auth/password-reset/confirm`.
///
/// `204` once the password is changed (every session of the account ends, so
/// the person signs in again); `410` when the link no longer works; `422` with
/// the unmet rules when the password is too weak (the link keeps working).
#[utoipa::path(
    post,
    path = "/auth/password-reset/confirm",
    request_body = ResetConfirmBody,
    responses(
        (status = 204, description = "Password changed; every session of the account ended"),
        (status = 410, description = "The link is invalid, used or expired", body = ErrorResponse),
        (status = 422, description = "The password does not meet the policy", body = ErrorResponse),
        (status = 429, description = "Rate-limited; carries Retry-After", body = ErrorResponse),
    ),
    tag = "auth",
)]
pub async fn confirm_handler(
    State(state): State<ResetState>,
    ClientIp(client_ip): ClientIp,
    headers: HeaderMap,
    Json(body): Json<ResetConfirmBody>,
) -> Response {
    if let Some(response) = state.limit(&[format!("ip:{client_ip}")]) {
        return response;
    }
    match state.service.reset(&body.token, &body.password).await {
        Ok(admin) => {
            state
                .audit(
                    AuditEventType::PasswordResetCompleted,
                    Some(admin),
                    None,
                    &client_ip,
                    &headers,
                )
                .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(ResetError::InvalidToken) => invalid_token(),
        Err(ResetError::WeakPassword(e)) => {
            let needs = e
                .unmet
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            ApiError::validation(BTreeMap::from([(
                "password".to_string(),
                format!("needs {needs}"),
            )]))
            .into_response()
        }
        Err(ResetError::MailUnavailable) => mail_unavailable(),
        Err(e) => ApiError::internal(format!("password reset confirm: {e}")).into_response(),
    }
}
