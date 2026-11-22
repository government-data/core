//! `updater/v1` (decision 0003): what the installation's update agent asks
//! quality-data, on Traefik's internal entry point with its certificate.
//!
//! | Call                                   | Does                                     |
//! | -------------------------------------- | ---------------------------------------- |
//! | `PUT /updater/v1/state`                | the agent's state, kept for the page     |
//! | `GET /updater/v1/orders`               | the policy and the pending request       |
//! | `PUT /updater/v1/maintenance`          | maintenance begins (`active`) or ends    |
//! | `PUT /updater/v1/modules`              | the installed modules, the whole list    |
//! | `GET /updater/v1/drain?horizon_secs=N` | what still runs, and whether it drained  |
//! | `GET /updater/v1/version`              | the version that answers                 |
//!
//! They mean what the host daemon's `/internal/*` routes mean, and replace
//! them; those stay, on their own port with their token, for the daemons
//! that still call them. Only the certificate of an approved machine of
//! role `updater` (`server updater-certificate`), passed by Traefik in
//! `X-Forwarded-Tls-Client-Cert`, is accepted: no session, no CSRF. A
//! refusal is `{"error": …}`, as the agent reads it. Neither the request
//! nor the certificate is logged.

use std::time::Duration;

use application::{
    InstalledModule, MachinesService, MaintenanceError, UpdatesError, UpdatesService,
};
use auth_gateway::http::client_cert::{ClientCertificateError, ClientCertificates};
use auth_gateway::MachineRole;
use axum::extract::{Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

/// The protocol's prefix, as the API sees it (Traefik strips `/api`).
pub const PREFIX: &str = "/updater/v1";

/// "Scheduled soon" when the agent does not say: within 30 minutes.
const DEFAULT_HORIZON: Duration = Duration::from_secs(30 * 60);

/// Who may call: the machines and whether the certificate header is read.
#[derive(Clone)]
struct Gate {
    machines: MachinesService,
    certificates: ClientCertificates,
}

/// Mount `updater/v1`; `certificates` says whether the client certificate
/// header is trusted.
pub fn routes(
    updates: UpdatesService,
    machines: MachinesService,
    certificates: ClientCertificates,
) -> Router {
    let gate = Gate {
        machines,
        certificates,
    };
    Router::new()
        .route("/updater/v1/state", put(state))
        .route("/updater/v1/orders", get(orders))
        .route("/updater/v1/maintenance", put(maintenance))
        .route("/updater/v1/modules", put(modules))
        .route("/updater/v1/drain", get(drain))
        .route("/updater/v1/version", get(version))
        .with_state(updates)
        .layer(axum::middleware::from_fn_with_state(gate, require_updater))
}

/// Only the update agent's certificate passes.
async fn require_updater(State(gate): State<Gate>, req: Request, next: Next) -> Response {
    let presented = match gate.certificates.read(req.headers()) {
        Ok(presented) => presented,
        Err(ClientCertificateError::NotTrusted) => {
            let why = texts::tr!("api.machines.certificateNotTrusted");
            return refusal(StatusCode::SERVICE_UNAVAILABLE, why);
        }
        Err(_) => return not_the_agent(),
    };
    match gate.machines.caller(&presented, MachineRole::Updater).await {
        Ok(Some(_)) => next.run(req).await,
        Ok(None) => not_the_agent(),
        Err(e) => {
            tracing::error!("updater/v1: the machine of a certificate: {e:?}");
            unavailable()
        }
    }
}

/// `PUT /updater/v1/state`: the agent's state document, stored as it is;
/// what is new in it is notified.
async fn state(State(updates): State<UpdatesService>, Json(body): Json<Value>) -> Response {
    match updates.report(body).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(UpdatesError::Invalid(field, e)) => {
            refusal(StatusCode::BAD_REQUEST, format!("{field}: {e}"))
        }
        Err(e) => {
            tracing::error!("updater/v1 state: {e}");
            unavailable()
        }
    }
}

/// `GET /updater/v1/orders`: `{policy, request}`.
async fn orders(State(updates): State<UpdatesService>) -> Response {
    match updates.orders().await {
        Ok(orders) => Json(orders).into_response(),
        Err(e) => logged_unavailable("orders", &e),
    }
}

#[derive(Debug, Deserialize)]
struct MaintenanceBody {
    active: bool,
    #[serde(default)]
    reason: String,
    /// How long it lasts if nobody ends it (renewed by asking again).
    #[serde(default)]
    secs: u64,
}

/// `PUT /updater/v1/maintenance`: `{"active": true, "reason", "secs"}` or
/// `{"active": false}`; the maintenance in force after it.
async fn maintenance(
    State(updates): State<UpdatesService>,
    Json(body): Json<MaintenanceBody>,
) -> Response {
    if !body.active {
        return match updates.end().await {
            Ok(()) => Json(json!({ "maintenance": null })).into_response(),
            Err(e) => logged_unavailable("maintenance", &e),
        };
    }
    let lasts = Duration::from_secs(body.secs);
    match updates.begin(&body.reason, lasts).await {
        Ok(m) => Json(json!({ "maintenance": m })).into_response(),
        Err(MaintenanceError::Invalid(why)) => refusal(StatusCode::BAD_REQUEST, why),
        Err(MaintenanceError::Unavailable(e)) => logged_unavailable("maintenance", &e),
    }
}

#[derive(Debug, Deserialize)]
struct ModulesBody {
    modules: Vec<InstalledModule>,
}

/// `PUT /updater/v1/modules`: `{"modules": [{name, version, drain_path}]}`,
/// every installed module; it replaces the list before it.
async fn modules(State(updates): State<UpdatesService>, Json(body): Json<ModulesBody>) -> Response {
    match updates.set_modules(body.modules).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(UpdatesError::Invalid(field, e)) => {
            refusal(StatusCode::BAD_REQUEST, format!("{field}: {e}"))
        }
        Err(e) => logged_unavailable("modules", &e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
struct DrainQuery {
    /// "Scheduled soon": within how many seconds.
    horizon_secs: Option<u64>,
}

/// `GET /updater/v1/drain?horizon_secs=N`: `{drained, running, deploying,
/// scheduled_soon}`, with `queued`, the modules that did not answer
/// (`not_answering`) and the maintenance in force.
async fn drain(State(updates): State<UpdatesService>, Query(q): Query<DrainQuery>) -> Response {
    let horizon = q.horizon_secs.map_or(DEFAULT_HORIZON, Duration::from_secs);
    let status = updates.drain_status(horizon).await;
    let maintenance = match updates.active().await {
        Ok(m) => m,
        Err(e) => return logged_unavailable("drain", &e),
    };
    Json(json!({
        "drained": status.is_drained(),
        "running": status.running,
        "deploying": status.deploying,
        "queued": status.queued,
        "scheduled_soon": status.scheduled_soon,
        "not_answering": &status.not_answering,
        "maintenance": maintenance,
    }))
    .into_response()
}

/// `GET /updater/v1/version`: `{version}`.
async fn version() -> Json<Value> {
    Json(json!({ "version": domain::VERSION }))
}

fn refusal(status: StatusCode, why: impl Into<String>) -> Response {
    (status, Json(json!({ "error": why.into() }))).into_response()
}

fn not_the_agent() -> Response {
    refusal(
        StatusCode::UNAUTHORIZED,
        texts::tr!("api.updater.notTheAgent"),
    )
}

fn unavailable() -> Response {
    let why = texts::tr!("api.updater.unavailable");
    refusal(StatusCode::SERVICE_UNAVAILABLE, why)
}

fn logged_unavailable(call: &str, e: &str) -> Response {
    tracing::error!("updater/v1 {call}: {e}");
    unavailable()
}
