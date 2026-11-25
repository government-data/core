//! `updater/v1`'s calls for a module's purge and the agent's own audit
//! (quality-sdk decision 0009), on the internal entry point, for the update
//! agent's certificate alone, as the rest of [`crate::updater_v1`]:
//!
//! | Call                               | Does                                              |
//! | ---------------------------------- | ------------------------------------------------- |
//! | `DELETE /updater/v1/modules/{name}` | deletes its clients, its managed policies and    |
//! |                                    | their assignments, and its actions from the catalog |
//! | `POST /updater/v1/audit`           | records an act of the agent in the audit chain    |
//!
//! A purge is refused while the module is installed: the agent removes it
//! first. Both answer only once what they did is recorded, so the agent
//! knows a purge was audited before it deletes anything.

use application::{MachinesService, PlatformService, UpdatesService};
use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::AuthorizationServer;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, post};
use axum::{Extension, Json, Router};
use domain::AuditEventType;
use serde::Deserialize;

use crate::auth::ClientIp;
use crate::updater_v1::{refusal, require_updater, unavailable, Caller, Gate};
use crate::updater_v1_clients::audit_changes;

/// The longest error text kept from the agent.
const MAX_ERROR: usize = 1000;

#[derive(Clone)]
struct Purge {
    provider: Option<AuthorizationServer>,
    platform: PlatformService,
    updates: UpdatesService,
}

/// Mount the calls.
pub fn routes(
    provider: Option<AuthorizationServer>,
    platform: PlatformService,
    updates: UpdatesService,
    machines: MachinesService,
    certificates: ClientCertificates,
) -> Router {
    let state = Purge {
        provider,
        platform,
        updates,
    };
    Router::new()
        .route("/updater/v1/modules/{module}", delete(purge))
        .route("/updater/v1/audit", post(audit))
        .with_state(state)
        .layer(axum::middleware::from_fn_with_state(
            Gate::new(machines, certificates),
            require_updater,
        ))
}

/// `DELETE /updater/v1/modules/{module}`: `204`, also when there was
/// nothing left to delete.
async fn purge(
    State(p): State<Purge>,
    Extension(caller): Extension<Caller>,
    Path(module): Path<String>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
) -> Response {
    match p.updates.installed_modules().await {
        Ok(installed) if installed.iter().any(|m| m.name == module) => {
            return refusal(
                StatusCode::CONFLICT,
                texts::tr!("api.updater.purgeInstalled", name = module),
            )
        }
        Ok(_) => {}
        Err(e) => return logged(&e),
    }
    let changes = match &p.provider {
        Some(provider) => match provider.delete_module_clients(&module).await {
            Ok(changes) => changes,
            Err(e) => return logged(&format!("{e:?}")),
        },
        None => Vec::new(),
    };
    let origin = crate::machines::origin(ip, &headers);
    audit_changes(&caller, &module, "", &changes, &origin).await;
    match p.platform.purge_module_permissions(&module).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => logged(&e.to_string()),
    }
}

/// `POST /updater/v1/audit`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentEvent {
    event: String,
    module: String,
    version: String,
    outcome: String,
    #[serde(default)]
    step: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// `POST /updater/v1/audit`: `204` once recorded; `503` when it could not
/// be, so a purge does not start.
async fn audit(
    Extension(caller): Extension<Caller>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(body): Json<AgentEvent>,
) -> Response {
    let Some(event) = event_type(&body.event) else {
        return refusal(StatusCode::BAD_REQUEST, texts::tr!("api.updater.badEvent"));
    };
    let words_ok = [Some(&body.module), Some(&body.version), Some(&body.outcome)]
        .into_iter()
        .chain([body.step.as_ref()])
        .flatten()
        .all(|w| is_word(w));
    if !words_ok {
        return refusal(StatusCode::BAD_REQUEST, texts::tr!("api.updater.badEvent"));
    }
    let mut detail = format!("version:{} outcome:{}", body.version, body.outcome);
    if let Some(step) = &body.step {
        detail.push_str(&format!(" step:{step}"));
    }
    if let Some(error) = &body.error {
        let error: String = error
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(MAX_ERROR)
            .collect();
        detail.push_str(&format!(" error:{error}"));
    }
    let origin = crate::machines::origin(ip, &headers);
    match caller
        .machines
        .module_event(event, &caller.machine, &body.module, detail, &origin)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => logged(&format!("{e:?}")),
    }
}

/// The agent's events, as the decision names them.
fn event_type(name: &str) -> Option<AuditEventType> {
    Some(match name {
        "ModuleAdded" => AuditEventType::ModuleAdded,
        "ModuleRemoved" => AuditEventType::ModuleRemoved,
        "ModulePurgeStarted" => AuditEventType::ModulePurgeStarted,
        "ModulePurged" => AuditEventType::ModulePurged,
        "ModuleSecretsRotated" => AuditEventType::ModuleSecretsRotated,
        _ => return None,
    })
}

/// A module name, a version, an outcome or a step: printable, no space, up
/// to 100.
fn is_word(w: &str) -> bool {
    !w.is_empty()
        && w.len() <= 100
        && w.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
}

fn logged(e: &str) -> Response {
    tracing::error!("updater/v1 purge or audit: {e}");
    unavailable()
}
