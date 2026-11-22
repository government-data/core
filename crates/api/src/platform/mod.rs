//! The HTTP boundary of the platform: who I am, global secrets and the
//! updates.
//!
//! Everything here is for people with a session (cookie, and CSRF on
//! mutations, through [`require_session`]). The capability each operation
//! needs is decided by [`PlatformService`] with the session's actor; here HTTP
//! is only translated.

use application::{PlatformError, PlatformService};
use auth_gateway::SessionService;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use iam::Actor;
use platform::SecretId;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::session::{require_session, CurrentSession};

mod secrets;
mod updates;

use secrets::*;
use updates::*;

/// Mount the platform's routes.
pub fn routes(platform: PlatformService, sessions: SessionService) -> Router {
    Router::new()
        .route("/access/me", get(me))
        // Global secrets, without their values: those the actor may manage.
        .route("/secrets", get(list_secrets).post(create_secret))
        .route(
            "/secrets/{id}",
            axum::routing::patch(update_secret).delete(delete_secret),
        )
        .route("/updates", get(get_updates))
        .route("/updates/policy", put(set_update_policy))
        .route("/updates/request", post(request_update))
        .with_state(platform)
        .layer(axum::middleware::from_fn_with_state(
            sessions,
            require_session,
        ))
}

pub(crate) fn to_api(e: PlatformError) -> ApiError {
    match e {
        PlatformError::NotFound => {
            ApiError::not_found(texts::tr!("api.platform.notFoundOrNoAccess"))
        }
        PlatformError::Forbidden => {
            ApiError::forbidden("forbidden", texts::tr!("api.platform.roleForbids"))
        }
        PlatformError::Invalid(m) => ApiError::bad_request("invalid", m),
        PlatformError::Conflict(m) => ApiError::conflict("conflict", m),
        PlatformError::Internal(m) => ApiError::internal(format!("platform: {m}")),
    }
}

type ApiResult<T> = Result<T, ApiError>;

/// The current session's actor, with its policies.
pub(crate) async fn actor(
    platform: &PlatformService,
    session: &CurrentSession,
) -> ApiResult<Actor> {
    platform
        .actor(
            &session.admin_id,
            &session.role,
            session.service_account.as_deref(),
        )
        .await
        .map_err(to_api)
}

/// Who I am and what I can do (so the UI shows its buttons).
async fn me(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
) -> ApiResult<Json<Value>> {
    let actor = actor(&platform, &session).await?;
    Ok(Json(json!({
        "admin_id": actor.admin_id,
        "is_admin": actor.is_admin,
        "service_account": actor.service_account,
        // The actions it may do on some resource (ADR 0016), for the UI.
        "actions": actor.actions(),
        "version": domain::VERSION,
        "edition": platform.edition(),
        // The platform's time zone (decision 0079), UTC when never set: the
        // default of a new schedule's form.
        "platform_timezone": platform.platform_timezone().await,
        // Maintenance (everyone) and new versions (administrators).
        "update_notice": platform.update_notice(&actor).await,
    })))
}
