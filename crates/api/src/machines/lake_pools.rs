//! Settings → Lake, the lake's pools (decision 0029, points 5 and 6; 0066
//! step 11, the user's rulings 1 to 3 of 2026-11-14), for whoever may do
//! `admin:ManageMachines`. A node's command is asked as any machine's,
//! `POST /machines/join-tokens` with the role `lake`, the pool and the node.
//!
//! | Route                     | Does                                          |
//! | ------------------------- | --------------------------------------------- |
//! | `GET /lake/pools`         | every pool, its nodes and which have joined   |
//! | `PUT /lake/pools/{name}`  | defines the pool (`main`) before any node     |

use application::{MachinesError, MachinesService, PoolError, PoolNodes};
use auth_gateway::{PoolRefusal, MAX_POOL_DRIVES, MAX_POOL_NODES, MIN_POOL_DRIVES, MIN_POOL_NODES};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use domain::AdminId;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{epoch, invalid, origin};
use crate::auth::ClientIp;
use crate::error::ApiError;
use crate::session::CurrentSession;

/// A pool's node, and the machine that joined as it.
#[derive(Debug, Serialize, ToSchema)]
pub struct PoolNodeOut {
    #[schema(example = "lake-a")]
    name: String,
    /// The machine's state once it joined (`pending`, `approved` or
    /// `draining`); absent until it does.
    state: Option<String>,
    /// The machine's id, for Settings → Execution.
    machine_id: Option<String>,
}

/// A pool as defined.
#[derive(Debug, Serialize, ToSchema)]
pub struct LakePoolOut {
    #[schema(example = "main")]
    name: String,
    /// In order.
    nodes: Vec<PoolNodeOut>,
    /// The drives of each node.
    drives: u16,
    /// In Unix epoch seconds.
    defined_at: i64,
}

impl From<PoolNodes> for LakePoolOut {
    fn from(p: PoolNodes) -> Self {
        let nodes = p.nodes.into_iter().map(|(name, machine)| PoolNodeOut {
            name,
            state: machine.as_ref().map(|m| m.state.as_str().to_string()),
            machine_id: machine.map(|m| m.id.as_str().to_string()),
        });
        Self {
            name: p.pool.layout.name().to_string(),
            nodes: nodes.collect(),
            drives: p.pool.layout.drives(),
            defined_at: epoch(p.pool.defined_at),
        }
    }
}

/// The lake's pools.
#[derive(Debug, Serialize, ToSchema)]
pub struct LakePoolsOut {
    pools: Vec<LakePoolOut>,
}

/// A pool's layout.
#[derive(Debug, Deserialize, ToSchema)]
pub struct LakePoolIn {
    /// The nodes' names, in order: 4 to 16, each a host's name its peers
    /// resolve (lowercase letters, digits and hyphens, not at either end).
    #[schema(example = json!(["lake-a", "lake-b", "lake-c", "lake-d"]))]
    nodes: Vec<String>,
    /// The drives of each node, 1 to 16.
    #[schema(example = 2)]
    drives: u16,
}

/// `GET /lake/pools`: every pool, its nodes in order and which have joined.
#[utoipa::path(
    get,
    path = "/lake/pools",
    responses(
        (status = 200, description = "The pools", body = LakePoolsOut),
        (status = 401, description = "No valid session"),
        (status = 403, description = "Without admin:ManageMachines"),
    ),
    tag = "machines",
)]
pub async fn pools_handler(State(machines): State<MachinesService>) -> Response {
    match machines.lake_pools().await {
        Ok(pools) => Json(LakePoolsOut {
            pools: pools.into_iter().map(Into::into).collect(),
        })
        .into_response(),
        Err(e) => error(e),
    }
}

/// `PUT /lake/pools/{name}`: defines the pool, `main`, before any of its
/// nodes exists; again in place until one joins.
#[utoipa::path(
    put,
    path = "/lake/pools/{name}",
    params(("name" = String, Path, description = "The pool's name: main")),
    request_body = LakePoolIn,
    responses(
        (status = 200, description = "Defined", body = LakePoolOut),
        (status = 403, description = "Without admin:ManageMachines, or missing CSRF header"),
        (status = 409, description = "A node of the pool has joined"),
        (status = 422, description = "A name, the nodes or the drives out of form"),
    ),
    tag = "machines",
)]
pub async fn define_pool_handler(
    State(machines): State<MachinesService>,
    Extension(session): Extension<CurrentSession>,
    Path(name): Path<String>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(body): Json<LakePoolIn>,
) -> Response {
    let by = AdminId::new(&session.admin_id);
    let defined = machines
        .define_pool(&name, &body.nodes, body.drives, &by, &origin(ip, &headers))
        .await;
    match defined {
        Ok(pool) => Json(LakePoolOut::from(pool)).into_response(),
        Err(e) => error(e),
    }
}

fn error(e: PoolError) -> Response {
    let nodes = |m| invalid("nodes", m);
    match e {
        PoolError::Layout(PoolRefusal::Name) | PoolError::NotMain => {
            invalid("name", texts::tr!("api.machines.poolNotMain"))
        }
        PoolError::Layout(PoolRefusal::TooFewNodes | PoolRefusal::TooManyNodes) => {
            nodes(texts::tr!(
                "api.machines.poolNodes",
                min = MIN_POOL_NODES,
                max = MAX_POOL_NODES
            ))
        }
        PoolError::Layout(PoolRefusal::NodeName) => nodes(texts::tr!("api.machines.poolNodeName")),
        PoolError::Layout(PoolRefusal::RepeatedNode) => {
            nodes(texts::tr!("api.machines.poolRepeatedNode"))
        }
        PoolError::Layout(PoolRefusal::Drives) => invalid(
            "drives",
            texts::tr!("api.machines.poolDrives", max = MAX_POOL_DRIVES),
        ),
        PoolError::Layout(PoolRefusal::TooFewDrives) => invalid(
            "drives",
            texts::tr!("api.machines.poolTooFewDrives", min = MIN_POOL_DRIVES),
        ),
        PoolError::Joined => {
            ApiError::conflict("pool_joined", texts::tr!("api.machines.poolJoined")).into_response()
        }
        PoolError::Unavailable => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_pools",
            texts::tr!("api.machines.poolsUnavailable"),
        )
        .into_response(),
        PoolError::Backend(e) => ApiError::internal(format!("pools: {e}")).into_response(),
    }
}

/// What a pool node's token refusal says, on its field.
pub(super) fn node_token_error(e: MachinesError) -> Option<Response> {
    let (field, message) = match e {
        MachinesError::Invalid("pool") => ("pool", texts::tr!("api.machines.poolNotDefined")),
        MachinesError::Invalid("node") => ("node", texts::tr!("api.machines.invalidNode")),
        MachinesError::Invalid("drives") => ("drives", texts::tr!("api.machines.poolDrivesCount")),
        _ => return None,
    };
    Some(invalid(field, message))
}
