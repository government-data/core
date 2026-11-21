//! API layer — the HTTP boundary.
//!
//! Depends on `application` (and `domain` for its types) and on `axum`. It
//! translates HTTP requests into application calls and back; it holds no
//! business logic and no storage concerns.

use std::sync::Arc;

use application::{HealthService, PlatformService, SettingsService};
use auth_gateway::AdminRepository;
use auth_gateway::{AuditService, OAuthLoginService, SessionService, UsersService};
use auth_gateway::{LoginService, PasswordResetService};
use axum::{extract::State, http::StatusCode, routing::get, Router};
use domain::Readiness;

pub mod audit;
pub mod cors;
pub mod iam;
pub mod internal;
pub mod language;
pub mod machines;
pub mod openapi;
pub mod platform;
pub mod settings;
pub mod tenancy;
pub mod updater_v1;

pub use auth_gateway::http::{
    auth, captcha, central, oauth, password_reset, rbac, session, users, verify,
};
pub use http_kit::{error, rate_limit, telemetry};
use oauth::OAuthRedirects;
pub use openapi::ApiDoc;
use rate_limit::RateLimitConfig;

/// Build the HTTP router, injecting the application services as state.
///
/// Each concern is a self-contained sub-router carrying its own state, merged
/// onto a stateless base — so adding a delivery surface (here, admin login)
/// never entangles it with another's state. New features add a `.merge(...)`
/// line as they land. The CORS layer wraps the whole router last, so it
/// applies (and answers preflight `OPTIONS`) uniformly across every route.
///
/// `oauth`, `settings` and `password_reset` are optional: `None` simply leaves
/// those endpoints unmounted (OAuth returns `404`).
// The composition root legitimately injects one argument per delivery surface;
// bundling them into a struct would only move the same wiring elsewhere.
#[allow(clippy::too_many_arguments)]
pub fn router(
    health: HealthService,
    login: LoginService,
    sessions: SessionService,
    audit: AuditService,
    admins: Arc<dyn AdminRepository>,
    oauth: Option<(OAuthLoginService, OAuthRedirects)>,
    platform: PlatformService,
    settings: Option<SettingsService>,
    password_reset: Option<(PasswordResetService, password_reset::ResetLinkBase)>,
    cors_allowed_origins: &[String],
    login_rate_limit: RateLimitConfig,
) -> Router {
    // Who builds each session's Actor (from memory): used by the routes that
    // require an action (`rbac::require_action`, ADR 0016).
    let resolver = iam::resolver(platform.clone());
    let mut router = Router::new()
        .merge(health_routes(health))
        .merge(auth::routes(
            login,
            sessions.clone(),
            audit.clone(),
            login_rate_limit,
        ))
        .merge(session::routes(
            sessions.clone(),
            audit.clone(),
            admins.clone(),
            session::AppVersion {
                version: domain::VERSION,
                commit: domain::COMMIT,
            },
        ))
        .merge(verify::routes(sessions.clone(), admins.clone()))
        .merge(audit::routes(audit.clone(), sessions.clone()))
        .merge(iam::routes(platform.clone(), sessions.clone()))
        .merge(platform::routes(platform, sessions.clone()));
    // Users: creating an account sends its activation link, which goes out
    // through the tenant's mail and is redeemed in the password form.
    if let (Some(settings), Some((service, links))) = (&settings, &password_reset) {
        router = router.merge(users::routes(
            UsersService::new(admins.clone(), service.clone()),
            Arc::new(settings.clone()),
            sessions.clone(),
            audit.clone(),
            links.clone(),
        ));
    }
    if let Some(settings) = settings {
        router = router.merge(settings::routes(settings, sessions.clone(), admins.clone()));
    }
    if let Some((service, links)) = password_reset {
        router = router.merge(password_reset::routes(
            service,
            audit.clone(),
            links,
            login_rate_limit,
        ));
    }
    if let Some((oauth, redirects)) = oauth {
        router = router.merge(oauth::routes(oauth, sessions.clone(), audit, redirects));
    }
    // CORS wraps every route; the request-context layer is added last so it is
    // the OUTERMOST wrapper — every request (CORS preflight included) runs inside
    // a `request` span with a trace id, echoed back in `x-request-id`.
    router
        .layer(axum::Extension(resolver))
        .layer(cors::layer(cors_allowed_origins))
        .layer(axum::middleware::from_fn(telemetry::request_context))
}

/// The liveness + readiness sub-router.
///
/// `/health` is *liveness* — the process is up and answering, with no dependency
/// check, so it always returns `200` and never flaps a container restart on a
/// transient database blip. `/ready` is *readiness* — it probes the database, so
/// an orchestrator can hold traffic off an instance that cannot serve yet.
fn health_routes(health: HealthService) -> Router {
    Router::new()
        .route("/health", get(liveness_handler))
        .route("/ready", get(readiness_handler))
        .with_state(health)
}

/// Liveness probe: `200 OK` whenever the process is running. No dependency is
/// checked, so a slow or briefly-unreachable database never turns into a restart.
#[utoipa::path(
    get,
    path = "/health",
    responses((status = 200, description = "The process is alive")),
    tag = "health",
)]
pub(crate) async fn liveness_handler() -> StatusCode {
    StatusCode::OK
}

/// Readiness probe: `200 OK` when ready, `503 Service Unavailable` otherwise.
///
/// Awaits the application service, which in turn probes the live database, so a
/// down or unreachable Postgres surfaces here as `503`.
#[utoipa::path(
    get,
    path = "/ready",
    responses(
        (status = 200, description = "Ready to serve"),
        (status = 503, description = "Not ready (database unreachable)"),
    ),
    tag = "health",
)]
pub(crate) async fn readiness_handler(State(health): State<HealthService>) -> StatusCode {
    match health.health().await.readiness {
        Readiness::Ready => StatusCode::OK,
        Readiness::NotReady => StatusCode::SERVICE_UNAVAILABLE,
    }
}
