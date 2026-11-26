//! `GET /v1/machines` (quality-sdk decision 0011, the user's ruling 56):
//! the installation's live machines, for a module that must tell a machine
//! that is gone (the orchestrator: a cluster whose machine the core no
//! longer lists is gone), as `/v1/modules` lists the modules (0015). With
//! a module's machine client token for the core
//! ([`crate::platform_v1::require_module_client`]). A machine is live from
//! when it joins until it is retired; a retired one is not listed.

use application::MachinesService;
use auth_gateway::{AuthorizationServer, MachineState};
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::platform_v1::require_module_client;

/// Mount `GET /v1/machines`; `provider` is `None` without
/// `OIDC_SIGNING_KEY`, and then the call is a 503.
pub fn routes(provider: Option<AuthorizationServer>, machines: MachinesService) -> Router {
    Router::new()
        .route("/v1/machines", get(live))
        .with_state(machines)
        .layer(axum::middleware::from_fn_with_state(
            provider,
            require_module_client,
        ))
}

/// `{"machines": [{"name", "role", "state"}]}`: every machine not retired,
/// by name (its certificate's CN), its role and its state (`pending`,
/// `approved` or `draining`).
async fn live(State(machines): State<MachinesService>) -> Response {
    match machines.list().await {
        Ok(list) => {
            let live: Vec<_> = list
                .iter()
                .filter(|m| m.state != MachineState::Retired)
                .map(|m| {
                    json!({ "name": m.name, "role": m.role.as_str(), "state": m.state.as_str() })
                })
                .collect();
            Json(json!({ "machines": live })).into_response()
        }
        Err(e) => {
            tracing::error!("v1 machines: {e:?}");
            crate::updater_v1::unavailable()
        }
    }
}
