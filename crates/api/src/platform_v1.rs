//! `/v1/…` (quality-sdk decision 0002, "Talking to each other"): what an
//! installed module asks the core, over Traefik's internal entry point,
//! with an access token of its machine client (`client_credentials`,
//! `<module>-machine`, quality-sdk decision 0009) issued for the audience
//! [`AUDIENCE`]. No session, no CSRF. Refusals are `{"error": …}`; neither
//! the token nor the request is logged.
//!
//! | Call               | Does                                                 |
//! | ------------------ | ---------------------------------------------------- |
//! | `GET /v1/modules`  | the installed modules, as the update agent told them |

use application::UpdatesService;
use auth_gateway::AuthorizationServer;
use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::updater_v1::refusal;

/// The audience a module's machine client asks its token for, to call
/// the core.
pub const AUDIENCE: &str = "core";

/// The machine client that calls, as its token names it.
#[derive(Debug, Clone)]
pub struct ModuleCaller {
    pub client: String,
}

/// Mount `/v1/…`; `provider` is `None` without `OIDC_SIGNING_KEY`, and
/// then every call is a 503.
pub fn routes(provider: Option<AuthorizationServer>, updates: UpdatesService) -> Router {
    Router::new()
        .route("/v1/modules", get(modules))
        .with_state(updates)
        .layer(axum::middleware::from_fn_with_state(
            provider,
            require_module_client,
        ))
}

/// Only a live machine token of this installation for [`AUDIENCE`] passes.
pub async fn require_module_client(
    State(provider): State<Option<AuthorizationServer>>,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(provider) = provider else {
        let why = texts::tr!("api.updater.noProvider");
        return refusal(StatusCode::SERVICE_UNAVAILABLE, why);
    };
    let client = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .and_then(|token| provider.machine_client(token, AUDIENCE));
    match client {
        Some(client) => {
            req.extensions_mut().insert(ModuleCaller { client });
            next.run(req).await
        }
        None => refusal(
            StatusCode::UNAUTHORIZED,
            texts::tr!("api.platform.notAModuleClient"),
        ),
    }
}

/// `GET /v1/modules`: `{"modules": [{name, version}]}`, every installed
/// module as the update agent last told them (quality-sdk decision 0015:
/// a module treats another as installed only while this lists it).
async fn modules(State(updates): State<UpdatesService>) -> Response {
    match updates.installed_modules().await {
        Ok(modules) => {
            let modules: Vec<_> = modules
                .iter()
                .map(|m| json!({ "name": m.name, "version": m.version }))
                .collect();
            Json(json!({ "modules": modules })).into_response()
        }
        Err(e) => {
            tracing::error!("v1 modules: {e}");
            crate::updater_v1::unavailable()
        }
    }
}
