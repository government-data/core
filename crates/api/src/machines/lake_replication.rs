//! Settings → Lake, the replication of the main lake into the replica
//! (decision 0066 step 11, the user's rulings of 2026-11-13), for whoever
//! may do `admin:ManageMachines`: whether it can run, and what the last
//! run left of each tenant's warehouse, made on the main lake first. It runs when keys are pasted and
//! by the periodic reconcile; no key is answered.
//!
//! | Route                  | Does                                          |
//! | ---------------------- | --------------------------------------------- |
//! | `GET /lake/replication` | whether it can run, and each warehouse's state |

use application::{LakeAdminError, MachinesService, ReplicationReadiness, ReplicationStatus};
use auth_gateway::{Lake, ReplicationOutcome, WarehouseState};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

use super::epoch;
use crate::error::ApiError;

/// One warehouse, as the last run left it.
#[derive(Debug, Serialize, ToSchema)]
pub struct WarehouseReplicationOut {
    #[schema(example = "qd-tenant-00000000000000000000000000000001")]
    warehouse: String,
    /// `configured`, `made` (made on the main lake, its replication
    /// waiting for the replica or its keys), `no_warehouse` (not on the
    /// main lake yet), `no_keys` or `failed`.
    #[schema(example = "configured")]
    state: &'static str,
    /// With `failed`: the step (`warehouse_bucket` and `warehouse_tables`,
    /// making the warehouse on the main lake; `source_bucket`, `replica_bucket`,
    /// `replica_retention`, `source_versioning`, `source_lifecycle`,
    /// `remote_target`, `rule`).
    step: Option<&'static str>,
    /// With `no_keys`: `main` or `replica`.
    lake: Option<&'static str>,
    /// With `failed`: what happened, in a few words.
    detail: Option<String>,
    /// When it was first configured, in Unix epoch seconds.
    configured_at: Option<i64>,
    /// When it was configured or tried last, in Unix epoch seconds.
    checked_at: i64,
}

/// The replication, as Settings → Lake shows it.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReplicationOut {
    /// `ready`, `no_address` (no main lake), `no_replica` (none, or
    /// several, approved) or `no_keys`.
    #[schema(example = "ready")]
    readiness: &'static str,
    /// With `no_keys`: `main` or `replica`.
    lake: Option<&'static str>,
    /// With `ready`: the main lake, the replica's node and the proxy the
    /// main lake reaches it through.
    source: Option<String>,
    replica: Option<String>,
    proxy: Option<String>,
    /// The replica's default retention (`COMPLIANCE`), in days.
    #[schema(example = 30)]
    retention_days: u32,
    /// After how many days the main lake's noncurrent versions expire.
    #[schema(example = 7)]
    noncurrent_days: u32,
    warehouses: Vec<WarehouseReplicationOut>,
}

fn lake_name(lake: &Lake) -> &'static str {
    match lake {
        Lake::Replica => "replica",
        Lake::Pool(_) => "main",
    }
}

impl From<WarehouseState> for WarehouseReplicationOut {
    fn from(w: WarehouseState) -> Self {
        let (state, step, lake, detail) = match &w.outcome {
            ReplicationOutcome::Configured => ("configured", None, None, None),
            ReplicationOutcome::NoWarehouse => ("no_warehouse", None, None, None),
            ReplicationOutcome::Made => ("made", None, None, None),
            ReplicationOutcome::NoKeys { lake } => ("no_keys", None, Some(lake_name(lake)), None),
            ReplicationOutcome::Failed { step, detail } => {
                ("failed", Some(step.as_str()), None, Some(detail.clone()))
            }
        };
        Self {
            warehouse: w.warehouse,
            state,
            step,
            lake,
            detail,
            configured_at: w.configured_at.map(epoch),
            checked_at: epoch(w.checked_at),
        }
    }
}

impl From<ReplicationStatus> for ReplicationOut {
    fn from(s: ReplicationStatus) -> Self {
        let mut out = Self {
            readiness: "ready",
            lake: None,
            source: None,
            replica: None,
            proxy: None,
            retention_days: auth_gateway::REPLICA_RETENTION_DAYS,
            noncurrent_days: auth_gateway::SOURCE_NONCURRENT_DAYS,
            warehouses: s.warehouses.into_iter().map(Into::into).collect(),
        };
        match s.readiness {
            ReplicationReadiness::Ready(lakes) => {
                out.source = Some(lakes.source);
                out.replica = Some(lakes.replica);
                out.proxy = Some(lakes.proxy);
            }
            ReplicationReadiness::NoAddress => out.readiness = "no_address",
            ReplicationReadiness::NoReplica(_) => out.readiness = "no_replica",
            ReplicationReadiness::NoKeys { lake } => {
                out.readiness = "no_keys";
                out.lake = Some(lake_name(&lake));
            }
        }
        out
    }
}

/// `GET /lake/replication`: whether the replication can run, and what the
/// last run left of each warehouse.
#[utoipa::path(
    get,
    path = "/lake/replication",
    responses(
        (status = 200, description = "Whether it can run, and each warehouse", body = ReplicationOut),
        (status = 401, description = "No valid session"),
        (status = 403, description = "Without admin:ManageMachines"),
        (status = 503, description = "This server cannot configure the replication"),
    ),
    tag = "machines",
)]
pub async fn replication_handler(State(machines): State<MachinesService>) -> Response {
    match machines.replication_status().await {
        Ok(status) => Json(ReplicationOut::from(status)).into_response(),
        Err(LakeAdminError::Backend(e)) => {
            ApiError::internal(format!("lake replication: {e}")).into_response()
        }
        Err(_) => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_lake_admin",
            texts::tr!("api.machines.lakeAdminUnavailable"),
        )
        .into_response(),
    }
}
