//! Settings → Lake, the main lake as the application sees it (decision
//! 0028, "Lake health"; 0066 step 11, the user's ruling 1 of 2026-11-13),
//! for whoever may do `admin:ManageMachines`: where it is reached and what
//! it answers, asked now with the pasted keys. No key is answered.
//!
//! | Route                  | Does                                     |
//! | ---------------------- | ---------------------------------------- |
//! | `GET /lake/main/state` | where the main lake is, and what it says |

use application::lake::LakeProbe;
use application::{LakeAdminError, MachinesService, MainLakeState};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

use crate::error::ApiError;

/// The main lake, as the application sees it now.
#[derive(Debug, Default, Serialize, ToSchema)]
pub struct MainLakeStateOut {
    /// `no_address`, `no_keys`, `answered`, `keys_refused` or
    /// `unreachable`.
    #[schema(example = "answered")]
    state: &'static str,
    /// Where the application reaches it (`LAKE_ENDPOINT`).
    #[schema(example = "https://lake-1")]
    endpoint: Option<String>,
    /// With `answered`: its drives online and offline.
    drives_online: Option<u32>,
    drives_offline: Option<u32>,
    /// With `keys_refused`: RustFS's code.
    #[schema(example = "SignatureDoesNotMatch")]
    code: Option<String>,
    /// With `unreachable`: what happened, as the connection said it.
    detail: Option<String>,
}

impl From<MainLakeState> for MainLakeStateOut {
    fn from(state: MainLakeState) -> Self {
        let MainLakeState::Probed { endpoint, probe } = state else {
            return Self {
                state: "no_address",
                ..Self::default()
            };
        };
        let endpoint = Some(endpoint);
        match probe {
            LakeProbe::NoKeys => Self {
                state: "no_keys",
                endpoint,
                ..Self::default()
            },
            LakeProbe::Answered {
                drives_online,
                drives_offline,
            } => Self {
                state: "answered",
                endpoint,
                drives_online: Some(drives_online),
                drives_offline: Some(drives_offline),
                ..Self::default()
            },
            LakeProbe::KeysRefused { code } => Self {
                state: "keys_refused",
                endpoint,
                code: Some(code),
                ..Self::default()
            },
            LakeProbe::Unreachable { detail } => Self {
                state: "unreachable",
                endpoint,
                detail: Some(detail),
                ..Self::default()
            },
        }
    }
}

/// `GET /lake/main/state`: where the main lake is reached, and what its
/// admin API answers now with the pasted keys.
#[utoipa::path(
    get,
    path = "/lake/main/state",
    responses(
        (status = 200, description = "The main lake and what it answered", body = MainLakeStateOut),
        (status = 401, description = "No valid session"),
        (status = 403, description = "Without admin:ManageMachines"),
        (status = 503, description = "This server cannot ask the lake"),
    ),
    tag = "machines",
)]
pub async fn main_state_handler(State(machines): State<MachinesService>) -> Response {
    match machines.main_lake_state().await {
        Ok(state) => Json(MainLakeStateOut::from(state)).into_response(),
        Err(LakeAdminError::Backend(e)) => {
            ApiError::internal(format!("main lake: {e}")).into_response()
        }
        Err(_) => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_lake_admin",
            texts::tr!("api.machines.lakeAdminUnavailable"),
        )
        .into_response(),
    }
}
