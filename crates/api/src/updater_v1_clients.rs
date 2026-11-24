//! `updater/v1`'s calls for a module's OIDC clients (quality-sdk decision
//! 0009), on the internal entry point, for the update agent's certificate
//! alone, as the rest of [`crate::updater_v1`]:
//!
//! | Call                                               | Does                                   |
//! | -------------------------------------------------- | -------------------------------------- |
//! | `PUT /updater/v1/modules/{module}/clients`         | registers or updates them, enabled     |
//! | `POST /updater/v1/modules/{module}/clients/disable` | disables them: no code, no token      |
//!
//! The body carries the secrets the agent made: it is never logged, and a
//! refusal never repeats it. What changed is recorded in the audit chain
//! with the agent's machine as the actor.

use application::UpdatesService;
use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::{AuthorizationServer, ClientChange, ModuleClientsError, ModuleClientsRequest};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{post, put};
use axum::{Extension, Json, Router};
use domain::AuditEventType;
use serde_json::json;

use crate::auth::ClientIp;
use crate::updater_v1::{refusal, require_updater, unavailable, Caller, Gate};

/// What the calls use: the provider (`None` without `OIDC_SIGNING_KEY`),
/// this installation's public origin and the installed modules.
#[derive(Clone)]
struct Clients {
    provider: Option<AuthorizationServer>,
    origin: String,
    updates: UpdatesService,
}

/// Mount the calls; `origin` is the public origin the redirect URIs go
/// under (`https://host`).
pub fn routes(
    provider: Option<AuthorizationServer>,
    origin: String,
    updates: UpdatesService,
    machines: application::MachinesService,
    certificates: ClientCertificates,
) -> Router {
    let state = Clients {
        provider,
        origin,
        updates,
    };
    Router::new()
        .route("/updater/v1/modules/{module}/clients", put(put_clients))
        .route(
            "/updater/v1/modules/{module}/clients/disable",
            post(disable_clients),
        )
        .with_state(state)
        .layer(axum::middleware::from_fn_with_state(
            Gate::new(machines, certificates),
            require_updater,
        ))
}

/// `PUT /updater/v1/modules/{module}/clients`: `{issuer, people_client_id,
/// machine_client_id}`.
async fn put_clients(
    State(clients): State<Clients>,
    Extension(caller): Extension<Caller>,
    Path(module): Path<String>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(body): Json<ModuleClientsRequest>,
) -> Response {
    let Some(provider) = &clients.provider else {
        return no_provider();
    };
    match provider
        .put_module_clients(&module, &body, &clients.origin)
        .await
    {
        Ok(done) => {
            audit(&clients, &caller, &module, &done.changes, ip, &headers).await;
            Json(json!({
                "issuer": done.issuer,
                "people_client_id": done.people_client_id,
                "machine_client_id": done.machine_client_id,
            }))
            .into_response()
        }
        Err(e) => refused(e),
    }
}

/// `POST /updater/v1/modules/{module}/clients/disable`: `204`.
async fn disable_clients(
    State(clients): State<Clients>,
    Extension(caller): Extension<Caller>,
    Path(module): Path<String>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
) -> Response {
    let Some(provider) = &clients.provider else {
        return no_provider();
    };
    match provider.disable_module_clients(&module).await {
        Ok(changes) => {
            audit(&clients, &caller, &module, &changes, ip, &headers).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => refused(e),
    }
}

/// Each change as an event of the audit chain: the client, the module's
/// version when the core knows it, the outcome.
pub(crate) async fn audit_changes(
    caller: &Caller,
    module: &str,
    version: &str,
    changes: &[ClientChange],
    origin: &application::Origin,
) {
    for change in changes {
        let (event, client) = match change {
            ClientChange::Registered(c) => (AuditEventType::ModuleClientRegistered, c),
            ClientChange::Changed(c) => (AuditEventType::ModuleClientChanged, c),
            ClientChange::Disabled(c) => (AuditEventType::ModuleClientDisabled, c),
            ClientChange::Deleted(c) => (AuditEventType::ModuleClientDeleted, c),
        };
        let detail = format!("client:{client} version:{version} outcome:ok");
        let recorded = caller
            .machines
            .module_event(event, &caller.machine, module, detail, origin)
            .await;
        if let Err(e) = recorded {
            tracing::warn!("updater/v1 clients: the audit could not be recorded: {e:?}");
        }
    }
}

async fn audit(
    clients: &Clients,
    caller: &Caller,
    module: &str,
    changes: &[ClientChange],
    ip: String,
    headers: &HeaderMap,
) {
    let version = installed_version(&clients.updates, module).await;
    let origin = crate::machines::origin(ip, headers);
    audit_changes(caller, module, &version, changes, &origin).await;
}

/// The module's version as last told, empty when it is not installed yet
/// (an `add` registers its clients before it tells the list).
pub(crate) async fn installed_version(updates: &UpdatesService, module: &str) -> String {
    updates
        .installed_modules()
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|m| m.name == module)
        .map(|m| m.version)
        .unwrap_or_default()
}

fn refused(e: ModuleClientsError) -> Response {
    match e {
        ModuleClientsError::Invalid(why) => refusal(StatusCode::BAD_REQUEST, why),
        ModuleClientsError::Taken(id) => refusal(
            StatusCode::CONFLICT,
            texts::tr!("api.updater.clientTaken", client = id),
        ),
        ModuleClientsError::Internal(e) => {
            tracing::error!("updater/v1 clients: {e}");
            unavailable()
        }
    }
}

fn no_provider() -> Response {
    refusal(
        StatusCode::SERVICE_UNAVAILABLE,
        texts::tr!("api.updater.noProvider"),
    )
}
