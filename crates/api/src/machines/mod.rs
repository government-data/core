//! Settings → Execution, the installation's machines (decisions 0029 and
//! 0066), for whoever may do `admin:ManageMachines`:
//!
//! | Route                            | Does                                        |
//! | -------------------------------- | ------------------------------------------- |
//! | `GET /machines`                  | every machine, retired ones too             |
//! | `GET /machines/metrics`          | their charts' series, from Mimir            |
//! | `POST /machines/join-tokens`     | a token for one machine to join, shown once |
//! | `POST /machines/{id}/approve`    | a pending machine takes work                |
//! | `POST /machines/{id}/drain`      | it finishes what it has and gets no more    |
//! | `POST /machines/{id}/retire`     | it is out, its certificate refused          |
//!
//! And Settings → Lake's replica admin keys ([`lake_admin`]), what the
//! replica answers with them ([`lake_replica`]) and the lake's pools
//! ([`lake_pools`]).
//!
//! And, without a session, what a new machine is installed with
//! ([`install`]), the route a machine joins with ([`join`]) and the one it
//! renews its certificate with ([`renew`]), Dolt's hold before it
//! restarts ([`dolt`]), the lake's gate ([`lake`]) and Grafana's alerts
//! ([`alerts`]). The rules are [`MachinesService`]'s; here HTTP is only
//! translated.

pub mod alerts;
pub mod dolt;
pub mod install;
pub mod join;
pub mod lake;
pub mod lake_admin;
pub mod lake_main;
pub mod lake_pools;
pub mod lake_replica;
pub mod lake_replication;
pub mod lifecycle;
pub mod metrics;
pub mod registry;
pub mod renew;

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use application::{Drives, DrivesError, MachinesError, MachinesService, Origin, MAX_DRIVES};
use auth_gateway::{Machine, MachineRole, SessionService};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use domain::AdminId;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::ClientIp;
use crate::error::ApiError;
use crate::rbac::require_action;
use crate::session::{require_session, CurrentSession};

/// Mount the administrators' routes for machines.
pub fn routes(machines: MachinesService, sessions: SessionService) -> Router {
    Router::new()
        .route("/machines", get(list_machines_handler))
        .route("/machines/metrics", get(metrics::metrics_handler))
        .route("/machines/join-tokens", post(issue_token_handler))
        .route("/machines/{id}/approve", post(lifecycle::approve_handler))
        .route("/machines/{id}/drain", post(lifecycle::drain_handler))
        .route("/machines/{id}/retire", post(lifecycle::retire_handler))
        .route(
            "/lake/replica/admin-keys",
            get(lake_admin::replica_admin_keys_handler)
                .put(lake_admin::set_replica_admin_keys_handler),
        )
        .route(
            "/lake/replica/state",
            get(lake_replica::replica_state_handler),
        )
        .route(
            "/lake/main/admin-keys",
            get(lake_admin::main_admin_keys_handler).put(lake_admin::set_main_admin_keys_handler),
        )
        .route("/lake/main/state", get(lake_main::main_state_handler))
        .route("/lake/pools", get(lake_pools::pools_handler))
        .route("/lake/pools/{name}", put(lake_pools::define_pool_handler))
        .route(
            "/lake/replication",
            get(lake_replication::replication_handler),
        )
        .with_state(machines)
        .layer(axum::middleware::from_fn_with_state(
            iam::catalog::MANAGE_MACHINES,
            require_action,
        ))
        .layer(axum::middleware::from_fn_with_state(
            sessions,
            require_session,
        ))
}

/// One machine.
#[derive(Debug, Serialize, ToSchema)]
pub struct MachineOut {
    id: String,
    /// Its certificate's `CN`.
    name: String,
    /// `worker`, `trino-coordinator`, `trino-worker`, `lake`,
    /// `lake-replica`, `app` (the application VM) or `updater`: its certificate's `OU`.
    role: String,
    pool: Option<String>,
    /// `pending`, `approved`, `draining` or `retired`.
    state: String,
    /// What its agent reports running.
    version: Option<String>,
    /// The serial of the certificate in use (lowercase hex).
    cert_serial: Option<String>,
    /// When that certificate ends, in Unix epoch seconds.
    cert_not_after: Option<i64>,
    /// A renewed certificate it has not presented yet: accepted too, and the
    /// one in use once presented.
    cert_serial_next: Option<String>,
    /// In Unix epoch seconds, as the other times.
    created_at: i64,
    approved_at: Option<i64>,
    retired_at: Option<i64>,
}

impl From<Machine> for MachineOut {
    fn from(m: Machine) -> Self {
        Self {
            id: m.id.as_str().to_string(),
            name: m.name,
            role: m.role.as_str().to_string(),
            pool: m.pool,
            state: m.state.as_str().to_string(),
            version: m.version,
            cert_serial: m.cert_serial,
            cert_not_after: m.cert_not_after.map(epoch),
            cert_serial_next: m.cert_serial_next,
            created_at: epoch(m.created_at),
            approved_at: m.approved_at.map(epoch),
            retired_at: m.retired_at.map(epoch),
        }
    }
}

/// The machines, and whether new ones can join.
#[derive(Debug, Serialize, ToSchema)]
pub struct MachinesOut {
    machines: Vec<MachineOut>,
    /// `false` while the installation has no CA: no token can be issued.
    can_join: bool,
}

/// What a join token is for.
#[derive(Debug, Deserialize, ToSchema)]
pub struct JoinTokenIn {
    /// The role of the machine that joins with it.
    #[schema(example = "worker")]
    role: String,
    /// Its pool: lowercase letters, digits and hyphens.
    #[schema(example = "default")]
    pool: Option<String>,
    /// For the role `lake` alone, required: the node of the pool the token
    /// is for, one of the pool's (`PUT /lake/pools/{name}`). Only a machine
    /// of that name joins with it, and the command names it (`--name`).
    #[schema(example = "lake-a")]
    node: Option<String>,
    /// The mounted paths of a lake replica's or a pool node's drives, for its command to
    /// carry a `--drive` for each: absolute, of letters, digits, `.`, `_`,
    /// `-` and `/`, each once, at most 16. Refused for a role that takes
    /// none. Left out or empty, the command has no `--drive` and the person
    /// adds them by hand.
    #[schema(example = json!(["/srv/disk1", "/srv/disk2"]))]
    #[serde(default)]
    drives: Vec<String>,
}

/// A join token: shown here once, never again.
#[derive(Debug, Serialize, ToSchema)]
pub struct JoinTokenOut {
    token: String,
    role: String,
    pool: Option<String>,
    /// The lake pool's node it is for.
    node: Option<String>,
    /// When it stops working (an hour), in Unix epoch seconds.
    expires_at: i64,
    /// The command that installs the machine, with its hashes; absent for
    /// a role nothing installs yet, or without an HTTPS public address.
    install: Option<install::JoinCommandOut>,
}

/// `GET /machines`: every machine, by name.
#[utoipa::path(
    get,
    path = "/machines",
    responses(
        (status = 200, description = "The machines", body = MachinesOut),
        (status = 401, description = "No valid session"),
        (status = 403, description = "Without admin:ManageMachines"),
    ),
    tag = "machines",
)]
pub async fn list_machines_handler(State(machines): State<MachinesService>) -> Response {
    match machines.list().await {
        Ok(list) => Json(MachinesOut {
            machines: list.into_iter().map(MachineOut::from).collect(),
            can_join: machines.can_join(),
        })
        .into_response(),
        Err(e) => error(e),
    }
}

/// `POST /machines/join-tokens`: a token for one machine of a role to join,
/// for an hour.
#[utoipa::path(
    post,
    path = "/machines/join-tokens",
    request_body = JoinTokenIn,
    responses(
        (status = 201, description = "The token, shown once", body = JoinTokenOut),
        (status = 403, description = "Without admin:ManageMachines, or missing CSRF header"),
        (status = 422, description = "Unknown role, bad pool, node or drives"),
        (status = 503, description = "The installation has no CA"),
    ),
    tag = "machines",
)]
pub async fn issue_token_handler(
    State(machines): State<MachinesService>,
    Extension(session): Extension<CurrentSession>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(body): Json<JoinTokenIn>,
) -> Response {
    let Some(role) = MachineRole::parse(&body.role) else {
        return invalid("role", texts::tr!("api.machines.invalidRole"));
    };
    let drives = match Drives::parse(role, &body.drives) {
        Ok(drives) => drives,
        Err(e) => return invalid("drives", drives_message(e)),
    };
    let by = AdminId::new(&session.admin_id);
    let pool = body.pool.as_deref().filter(|p| !p.is_empty());
    let from = origin(ip, &headers);
    let issued = if role == MachineRole::Lake {
        let (pool, node) = (pool.unwrap_or_default(), body.node.as_deref());
        let node = node.unwrap_or_default();
        let count = drives.paths().len();
        let issued = machines
            .issue_node_token(pool, node, count, &by, &from)
            .await;
        match issued {
            Err(e) => match lake_pools::node_token_error(e.clone()) {
                Some(refused) => return refused,
                None => Err(e),
            },
            ok => ok,
        }
    } else if body.node.as_deref().is_some_and(|n| !n.is_empty()) {
        return invalid("node", texts::tr!("api.machines.nodeNotTaken"));
    } else {
        machines.issue_join_token(role, pool, &by, &from).await
    };
    match issued {
        Ok(t) => (
            StatusCode::CREATED,
            Json(JoinTokenOut {
                install: machines.join_command(&t, &drives).map(Into::into),
                token: t.token,
                role: t.role.as_str().to_string(),
                pool: t.pool,
                node: t.node,
                expires_at: epoch(t.expires_at),
            }),
        )
            .into_response(),
        Err(e) => error(e),
    }
}

/// Where a request came from, for the audit log.
pub(crate) fn origin(ip: String, headers: &HeaderMap) -> Origin {
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    Origin { ip, user_agent }
}

pub(crate) fn invalid(field: &str, message: String) -> Response {
    let fields = BTreeMap::from([(field.to_string(), message)]);
    ApiError::validation(fields).into_response()
}

pub(crate) fn no_ca() -> Response {
    let message = texts::tr!("api.machines.noCa");
    ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "no_ca", message).into_response()
}

fn drives_message(e: DrivesError) -> String {
    match e {
        DrivesError::NotTaken => texts::tr!("api.machines.drivesNotTaken"),
        DrivesError::Invalid => texts::tr!("api.machines.invalidDrive"),
        DrivesError::TooMany => texts::tr!("api.machines.tooManyDrives", max = MAX_DRIVES),
        DrivesError::Repeated => texts::tr!("api.machines.repeatedDrive"),
    }
}

fn error(e: MachinesError) -> Response {
    match e {
        MachinesError::Invalid("pool") => invalid("pool", texts::tr!("api.machines.invalidPool")),
        MachinesError::Invalid("role") => invalid("role", texts::tr!("api.machines.invalidRole")),
        MachinesError::Invalid(field) => invalid(field, texts::tr!("api.machines.invalid")),
        MachinesError::NotFound => {
            ApiError::not_found(texts::tr!("api.machines.notFound")).into_response()
        }
        MachinesError::NotAllowed(state) => ApiError::conflict(
            "transition_not_allowed",
            texts::tr!("api.machines.notAllowed", state = state.as_str()),
        )
        .into_response(),
        MachinesError::Unavailable => no_ca(),
        MachinesError::Backend(e) => ApiError::internal(format!("machines: {e}")).into_response(),
    }
}

pub(crate) fn epoch(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
