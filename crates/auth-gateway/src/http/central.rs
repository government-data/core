//! Signing in with the email, without typing the company's address (online
//! edition).
//!
//! - `POST /auth/central` — at `entrar.<domain>`, outside every company: the
//!   email and password. Answers the companies where they worked, each with the
//!   address (and the pass) to sign in. Same limits, captcha and answers as
//!   `/auth/login`: a wrong password and an email that does not exist look the
//!   same.
//! - `POST /auth/handoff` — on the company's subdomain: redeems the pass for
//!   the session (each subdomain's cookies are its own).
//!
//! Outside the OpenAPI contract, like the demo: only the online edition mounts
//! them, and the web calls them directly.

use std::sync::Arc;
use std::time::SystemTime;

use crate::{AuditService, CentralLoginService, SessionService};
use crate::{LoginError, LoginRequest};
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use domain::{AuditEventType, Email, NewAuditEvent};
use serde::Deserialize;
use serde_json::json;

use crate::http::auth::ClientIp;
use crate::http::session::attach_session_cookies;
use http_kit::error::ApiError;
use http_kit::rate_limit::{RateLimitConfig, RateLimiter};

#[derive(Clone)]
struct Central {
    service: CentralLoginService,
    sessions: SessionService,
    audit: AuditService,
    rate_limit: Arc<RateLimiter>,
}

/// The two routes. They go outside `api::router`, with the company (or none)
/// the tenant middleware resolved.
pub fn routes(
    service: CentralLoginService,
    sessions: SessionService,
    audit: AuditService,
    rate_limit: RateLimitConfig,
) -> Router {
    Router::new()
        .route("/auth/central", post(central))
        .route("/auth/handoff", post(handoff))
        .with_state(Central {
            service,
            sessions,
            audit,
            rate_limit: Arc::new(RateLimiter::new(rate_limit)),
        })
}

#[derive(Deserialize)]
struct CentralBody {
    email: String,
    password: String,
    #[serde(default)]
    captcha: Option<String>,
}

fn not_found() -> Response {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "Not found.").into_response()
}

fn too_many_attempts(retry_after_secs: Option<u64>) -> Response {
    let mut response = ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "too_many_attempts",
        "Too many attempts. Please try again later.",
    )
    .into_response();
    if let Some(value) = retry_after_secs.and_then(|s| HeaderValue::from_str(&s.to_string()).ok()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

async fn central(
    State(state): State<Central>,
    captcha: Option<axum::Extension<crate::http::captcha::Captcha>>,
    ClientIp(client_ip): ClientIp,
    headers: HeaderMap,
    Json(body): Json<CentralBody>,
) -> Response {
    // Only at the common entrance: inside a company one signs in at /auth/login.
    if tenancy::current().is_some() {
        return not_found();
    }
    if Email::parse(body.email.trim()).is_err() || body.password.is_empty() {
        return ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "Email and password are required.",
        )
        .into_response();
    }
    let now = SystemTime::now();
    let email = body.email.trim().to_ascii_lowercase();
    for key in [format!("ip:{client_ip}"), format!("acct:{email}")] {
        if let Err(exceeded) = state.rate_limit.check(&key, now) {
            tracing::warn!(key, "central login rate limit exceeded");
            return too_many_attempts(Some(exceeded.retry_after.as_secs()));
        }
    }
    let captcha = captcha.map(|axum::Extension(c)| c);
    if let Some(c) = &captcha {
        if c.required_for_login(&client_ip) {
            if let Err(e) = c.verify(body.captcha.as_deref(), &client_ip).await {
                return e.into_response();
            }
        }
    }
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let request = LoginRequest {
        email: body.email,
        password: body.password,
        client_ip: client_ip.clone(),
    };
    match state.service.sign_in(request, user_agent).await {
        Ok(entries) => {
            if let Some(c) = &captcha {
                c.record_success(&client_ip);
            }
            let companies: Vec<_> = entries
                .iter()
                .map(|e| {
                    json!({
                        "slug": e.record.tenant.slug,
                        "name": e.record.name,
                        "url": e.url,
                    })
                })
                .collect();
            Json(json!({ "companies": companies })).into_response()
        }
        Err(LoginError::InvalidCredentials) => {
            if let Some(c) = &captcha {
                c.record_failure(&client_ip);
            }
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "invalid_credentials",
                "Invalid email or password.",
            )
            .into_response()
        }
        Err(LoginError::TooManyAttempts { retry_after_secs }) => {
            too_many_attempts(retry_after_secs)
        }
        Err(LoginError::DirectoryUnavailable(msg)) => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "directory_unavailable",
            "The directory is not responding. Please try again later.",
        )
        .with_internal(format!("central login: directory unavailable: {msg}"))
        .into_response(),
        Err(LoginError::Internal(msg)) => {
            ApiError::internal(format!("central login: internal error: {msg}")).into_response()
        }
    }
}

#[derive(Deserialize)]
struct HandoffBody {
    token: String,
}

async fn handoff(
    State(state): State<Central>,
    ClientIp(client_ip): ClientIp,
    headers: HeaderMap,
    jar: CookieJar,
    Json(body): Json<HandoffBody>,
) -> Response {
    // The pass belongs to a company: it is redeemed on its subdomain.
    if tenancy::current().is_none() {
        return not_found();
    }
    let now = SystemTime::now();
    if let Err(exceeded) = state.rate_limit.check(&format!("ip:{client_ip}"), now) {
        return too_many_attempts(Some(exceeded.retry_after.as_secs()));
    }
    let admin = match state.service.redeem(&body.token).await {
        Ok(Some(admin)) => admin,
        Ok(None) => {
            return ApiError::new(
                StatusCode::UNAUTHORIZED,
                "invalid_handoff",
                "This sign-in link is invalid or has expired.",
            )
            .into_response()
        }
        Err(e) => {
            return ApiError::internal(format!("handoff: {e}")).into_response();
        }
    };
    let event = NewAuditEvent {
        event_type: AuditEventType::LoginSucceeded,
        admin_id: Some(admin.id.clone()),
        email_attempted: None,
        ip: client_ip,
        user_agent: headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
        occurred_at: now,
    };
    if let Err(e) = state.audit.record(event).await {
        tracing::warn!("handoff: failed to record audit event: {e}");
    }
    match state.sessions.start(admin.id.clone(), admin.role).await {
        Ok(issued) => (
            StatusCode::OK,
            attach_session_cookies(jar, &issued),
            Json(json!({ "admin_id": admin.id.as_str() })),
        )
            .into_response(),
        Err(e) => {
            ApiError::internal(format!("handoff: failed to issue session: {e}")).into_response()
        }
    }
}
