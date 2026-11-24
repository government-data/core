//! Settings → Lake, the replica as the application sees it (decision 0028,
//! "Lake health"; 0066 step 11), for whoever may do `admin:ManageMachines`:
//! which machine it is and what it answers, asked now with the pasted keys.
//! No key is answered.
//!
//! | Route                     | Does                                          |
//! | ------------------------- | --------------------------------------------- |
//! | `GET /lake/replica/state` | the replica's machine and what it answers now |

use application::lake::LakeProbe;
use application::{LakeAdminError, MachinesService, ReplicaState};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

use crate::error::ApiError;

/// The replica, as the application sees it now.
#[derive(Debug, Default, Serialize, ToSchema)]
pub struct ReplicaStateOut {
    /// `no_machine`, `several_machines`, `no_keys`, `answered`,
    /// `keys_refused` or `unreachable`.
    #[schema(example = "answered")]
    state: &'static str,
    /// The replica's machine, when there is one.
    #[schema(example = "replica-1")]
    machine: Option<String>,
    /// With `no_machine`: one that joined and waits to be approved.
    pending: Option<String>,
    /// With `several_machines`: the approved or draining ones.
    machines: Vec<String>,
    /// With `answered`: its drives online and offline.
    drives_online: Option<u32>,
    drives_offline: Option<u32>,
    /// With `keys_refused`: RustFS's code.
    #[schema(example = "SignatureDoesNotMatch")]
    code: Option<String>,
    /// With `unreachable`: what happened, as the connection said it.
    detail: Option<String>,
}

impl From<ReplicaState> for ReplicaStateOut {
    fn from(state: ReplicaState) -> Self {
        match state {
            ReplicaState::NoMachine { pending } => Self {
                state: "no_machine",
                pending,
                ..Self::default()
            },
            ReplicaState::SeveralMachines { names } => Self {
                state: "several_machines",
                machines: names,
                ..Self::default()
            },
            ReplicaState::Probed { machine, probe } => {
                let machine = Some(machine);
                match probe {
                    LakeProbe::NoKeys => Self {
                        state: "no_keys",
                        machine,
                        ..Self::default()
                    },
                    LakeProbe::Answered {
                        drives_online,
                        drives_offline,
                    } => Self {
                        state: "answered",
                        machine,
                        drives_online: Some(drives_online),
                        drives_offline: Some(drives_offline),
                        ..Self::default()
                    },
                    LakeProbe::KeysRefused { code } => Self {
                        state: "keys_refused",
                        machine,
                        code: Some(code),
                        ..Self::default()
                    },
                    LakeProbe::Unreachable { detail } => Self {
                        state: "unreachable",
                        machine,
                        detail: Some(detail),
                        ..Self::default()
                    },
                }
            }
        }
    }
}

/// `GET /lake/replica/state`: the replica's machine, and what its admin API
/// answers now with the pasted keys.
#[utoipa::path(
    get,
    path = "/lake/replica/state",
    responses(
        (status = 200, description = "The replica and what it answered", body = ReplicaStateOut),
        (status = 401, description = "No valid session"),
        (status = 403, description = "Without admin:ManageMachines"),
        (status = 503, description = "This server cannot ask the lake"),
    ),
    tag = "machines",
)]
pub async fn replica_state_handler(State(machines): State<MachinesService>) -> Response {
    match machines.replica_state().await {
        Ok(state) => Json(ReplicaStateOut::from(state)).into_response(),
        Err(LakeAdminError::Backend(e)) => {
            ApiError::internal(format!("lake replica: {e}")).into_response()
        }
        Err(_) => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_lake_admin",
            texts::tr!("api.machines.lakeAdminUnavailable"),
        )
        .into_response(),
    }
}
