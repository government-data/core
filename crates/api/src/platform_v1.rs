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
//! | `GET /v1/iam/effective?subject=<id>` | which of the module's actions a person holds |

use application::UpdatesService;
use auth_gateway::AuthorizationServer;
use axum::extract::{Query, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde::Deserialize;
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
    let effective = Effective {
        provider: provider.clone(),
        updates: updates.clone(),
    };
    Router::new()
        .route("/v1/modules", get(modules))
        .with_state(updates)
        .merge(
            Router::new()
                .route("/v1/iam/effective", get(iam_effective))
                .with_state(effective),
        )
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

/// What `iam/effective` reads: the provider (who the person is and what
/// they hold) and the modules' declared actions.
#[derive(Clone)]
struct Effective {
    provider: Option<AuthorizationServer>,
    updates: UpdatesService,
}

#[derive(Deserialize)]
struct SubjectQuery {
    /// The person's account id, as the `sub` of their tokens.
    subject: String,
}

/// `GET /v1/iam/effective?subject=<account id>`: `{"subject", "actions"}`,
/// which of the calling module's own actions (its manifest's
/// `permissions.actions`) the person holds now, by the core's IAM (deny
/// unless allowed, an explicit deny wins). The module is its machine
/// client's, never one it names. A module caches the answer at most 60 s.
/// No such account: `404`; a client no module registered: `403`.
async fn iam_effective(
    State(e): State<Effective>,
    Extension(caller): Extension<ModuleCaller>,
    Query(q): Query<SubjectQuery>,
) -> Response {
    let Some(provider) = e.provider else {
        return refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            texts::tr!("api.updater.noProvider"),
        );
    };
    let Some(module) = provider.client_module(&caller.client).await else {
        return refusal(StatusCode::FORBIDDEN, texts::tr!("api.platform.notAModule"));
    };
    let declared = match e.updates.declared_modules().await {
        Ok(declared) => declared,
        Err(err) => {
            tracing::error!("v1 iam/effective: {err}");
            return crate::updater_v1::unavailable();
        }
    };
    let actions: Vec<String> = declared
        .into_iter()
        .filter(|d| d.name == module && d.installed)
        .flat_map(|d| d.permissions.actions)
        .map(|a| a.name)
        .collect();
    match provider.held_actions(&q.subject, &actions).await {
        Ok(Some(held)) => Json(json!({ "subject": q.subject, "actions": held })).into_response(),
        Ok(None) => refusal(
            StatusCode::NOT_FOUND,
            texts::tr!("api.platform.noSuchSubject"),
        ),
        Err(err) => {
            tracing::error!("v1 iam/effective: what a person holds: {err}");
            crate::updater_v1::unavailable()
        }
    }
}
