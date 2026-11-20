//! Minimal RBAC (bead authapp-e00d47): a middleware that rejects a request
//! with `403` unless the authenticated session's role matches the one an
//! endpoint requires.
//!
//! Apply this **after** [`crate::http::session::require_session`] in the layer
//! chain — concretely, call `.layer()` for this middleware *before* calling
//! `.layer()` for `require_session`, since the later `.layer()` call becomes
//! the outer, first-executed wrapper (axum/tower run the last-added layer
//! first). `require_role` reads the [`CurrentSession`] `require_session`
//! populates; it does not authenticate the session itself.
//!
//! Adding a new role never touches this middleware — [`Role`] is a validated
//! string, not a closed set, so gating a new endpoint to a new role is a
//! one-line `require_role(Role::parse("editor")?)`, not a structural change.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use domain::Role;

use crate::http::session::CurrentSession;
use http_kit::error::ApiError;

/// Require that the authenticated session's role equals `required`, exactly.
///
/// Rejects with `403` if [`CurrentSession`] is missing (i.e. `require_session`
/// did not run first — a router-wiring bug, not a client error, but `403` is
/// the safe default over a `500`) or its role does not match.
pub async fn require_role(State(required): State<Role>, request: Request, next: Next) -> Response {
    let matches = request
        .extensions()
        .get::<CurrentSession>()
        .is_some_and(|current| current.role == required);

    if !matches {
        return ApiError::forbidden("forbidden", "You do not have permission to do that.")
            .into_response();
    }

    next.run(request).await
}

// --- IAM actions (ADR 0016) ---------------------------------------------------

/// Who builds a session's [`iam::Actor`] (with its policies). It is
/// implemented by whoever keeps the Actors in memory; this crate does not know
/// how.
#[async_trait::async_trait]
pub trait ActorSource: Send + Sync {
    async fn actor(&self, session: &CurrentSession) -> Result<iam::Actor, String>;
}

/// The application's [`ActorSource`], in the request extensions: an outer
/// layer puts it there (`axum::Extension`) and [`require_action`] uses it.
#[derive(Clone)]
pub struct ActorResolver(pub std::sync::Arc<dyn ActorSource>);

impl std::fmt::Debug for ActorResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ActorResolver")
    }
}

/// Require an action that acts on the installation as a whole
/// (`admin:ViewAudit`…): `403` unless the session's Actor may do it (`admin`
/// may do everything). Like [`require_role`], it goes *after*
/// `require_session` (add it first with `.layer()`); it also needs the
/// [`ActorResolver`] in the extensions. Without it (a wiring bug), `403`.
/// It leaves the [`iam::Actor`] in the extensions for the handler.
pub async fn require_action(
    State(action): State<&'static str>,
    mut request: Request,
    next: Next,
) -> Response {
    let forbidden = || {
        ApiError::forbidden("forbidden", "You do not have permission to do that.").into_response()
    };
    let (Some(session), Some(resolver)) = (
        request.extensions().get::<CurrentSession>().cloned(),
        request.extensions().get::<ActorResolver>().cloned(),
    ) else {
        return forbidden();
    };
    match resolver.0.actor(&session).await {
        Ok(actor) if actor.may(action) => {
            // The handler can use it without building it again.
            request.extensions_mut().insert(actor);
            next.run(request).await
        }
        Ok(_) => forbidden(),
        Err(error) => {
            tracing::warn!(%error, "could not build the session's actor");
            forbidden()
        }
    }
}
