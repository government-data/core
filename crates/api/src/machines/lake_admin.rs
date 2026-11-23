//! Settings → Lake, the lakes' platform admin keys (decision 0028, point
//! 2; 0066 step 11, the user's rulings of 2026-11-13), for whoever may do
//! `admin:ManageMachines`: `join.sh` shows the replica's once on the
//! replica, `infra/rustfs/admin-user.sh` the main lake's (`pool:main`) when
//! its stack is brought up, and a person pastes them here. The secret is
//! kept sealed and never answered.
//!
//! | Route                          | Does                                  |
//! | ------------------------------ | ------------------------------------- |
//! | `GET /lake/replica/admin-keys` | whether they are kept, and since when |
//! | `PUT /lake/replica/admin-keys` | keeps them, in place of any kept      |
//! | `GET /lake/main/admin-keys`    | the same, for the main lake           |
//! | `PUT /lake/main/admin-keys`    | the same, for the main lake           |

use application::{LakeAdminError, MachinesService};
use auth_gateway::{Lake, LakeAdminKeysKept};
use axum::extract::State;
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

/// What is kept of a lake's admin keys: never the secret.
#[derive(Debug, Serialize, ToSchema)]
pub struct LakeAdminKeysOut {
    /// Whether a person pasted them.
    set: bool,
    /// The access key, as `join.sh` showed it.
    #[schema(example = "qd-platform-admin")]
    access_key: Option<String>,
    /// When they were pasted last, in Unix epoch seconds.
    set_at: Option<i64>,
}

impl From<Option<LakeAdminKeysKept>> for LakeAdminKeysOut {
    fn from(kept: Option<LakeAdminKeysKept>) -> Self {
        Self {
            set: kept.is_some(),
            set_at: kept.as_ref().map(|k| epoch(k.set_at)),
            access_key: kept.map(|k| k.access_key),
        }
    }
}

/// The keys shown on the lake's server, pasted.
#[derive(Deserialize, ToSchema)]
pub struct LakeAdminKeysIn {
    #[schema(example = "qd-platform-admin")]
    access_key: String,
    /// 64 hex digits.
    secret_key: String,
}

impl std::fmt::Debug for LakeAdminKeysIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LakeAdminKeysIn")
            .field("access_key", &self.access_key)
            .finish_non_exhaustive()
    }
}

/// `GET /lake/replica/admin-keys`: whether the replica's admin keys are kept.
#[utoipa::path(
    get,
    path = "/lake/replica/admin-keys",
    responses(
        (status = 200, description = "What is kept, without the secret", body = LakeAdminKeysOut),
        (status = 401, description = "No valid session"),
        (status = 403, description = "Without admin:ManageMachines"),
        (status = 503, description = "This server keeps no lake keys"),
    ),
    tag = "machines",
)]
pub async fn replica_admin_keys_handler(State(machines): State<MachinesService>) -> Response {
    kept(&machines, &Lake::Replica).await
}

/// `GET /lake/main/admin-keys`: whether the main lake's admin keys are kept.
#[utoipa::path(
    get,
    path = "/lake/main/admin-keys",
    responses(
        (status = 200, description = "What is kept, without the secret", body = LakeAdminKeysOut),
        (status = 401, description = "No valid session"),
        (status = 403, description = "Without admin:ManageMachines"),
        (status = 503, description = "This server keeps no lake keys"),
    ),
    tag = "machines",
)]
pub async fn main_admin_keys_handler(State(machines): State<MachinesService>) -> Response {
    kept(&machines, &Lake::main()).await
}

async fn kept(machines: &MachinesService, lake: &Lake) -> Response {
    match machines.lake_admin_keys(lake).await {
        Ok(kept) => Json(LakeAdminKeysOut::from(kept)).into_response(),
        Err(e) => error(e),
    }
}

/// `PUT /lake/replica/admin-keys`: keeps the replica's admin keys, sealed,
/// in place of any kept; audited without them.
#[utoipa::path(
    put,
    path = "/lake/replica/admin-keys",
    request_body = LakeAdminKeysIn,
    responses(
        (status = 200, description = "Kept; the secret is not answered", body = LakeAdminKeysOut),
        (status = 403, description = "Without admin:ManageMachines, or missing CSRF header"),
        (status = 422, description = "An access key or a secret out of form"),
        (status = 503, description = "This server keeps no lake keys"),
    ),
    tag = "machines",
)]
pub async fn set_replica_admin_keys_handler(
    State(machines): State<MachinesService>,
    Extension(session): Extension<CurrentSession>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(body): Json<LakeAdminKeysIn>,
) -> Response {
    let by = AdminId::new(&session.admin_id);
    let from = origin(ip, &headers);
    set(&machines, &Lake::Replica, &body, &by, &from).await
}

/// `PUT /lake/main/admin-keys`: keeps the main lake's admin keys, sealed,
/// in place of any kept; audited without them.
#[utoipa::path(
    put,
    path = "/lake/main/admin-keys",
    request_body = LakeAdminKeysIn,
    responses(
        (status = 200, description = "Kept; the secret is not answered", body = LakeAdminKeysOut),
        (status = 403, description = "Without admin:ManageMachines, or missing CSRF header"),
        (status = 422, description = "An access key or a secret out of form"),
        (status = 503, description = "This server keeps no lake keys"),
    ),
    tag = "machines",
)]
pub async fn set_main_admin_keys_handler(
    State(machines): State<MachinesService>,
    Extension(session): Extension<CurrentSession>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(body): Json<LakeAdminKeysIn>,
) -> Response {
    let by = AdminId::new(&session.admin_id);
    let from = origin(ip, &headers);
    set(&machines, &Lake::main(), &body, &by, &from).await
}

async fn set(
    machines: &MachinesService,
    lake: &Lake,
    body: &LakeAdminKeysIn,
    by: &AdminId,
    from: &application::Origin,
) -> Response {
    let kept = machines
        .set_lake_admin_keys(lake, &body.access_key, &body.secret_key, by, from)
        .await;
    match kept {
        Ok(kept) => {
            replicate_after(machines, by, from);
            Json(LakeAdminKeysOut::from(Some(kept))).into_response()
        }
        Err(e) => error(e),
    }
}

/// Once keys are kept, the replication is configured with both lakes' keys
/// (0066 step 11, ruling 6), after the answer, by the person who pasted
/// them; it does nothing until both lakes are known and both keys kept.
fn replicate_after(machines: &MachinesService, by: &AdminId, from: &application::Origin) {
    let (machines, by, from) = (machines.clone(), by.clone(), from.clone());
    tokio::spawn(async move {
        match machines.replicate(Some((&by, &from))).await {
            Ok(_) | Err(LakeAdminError::Unavailable) => {}
            Err(e) => tracing::warn!(error = ?e, "lake replication after the keys were kept"),
        }
    });
}

fn error(e: LakeAdminError) -> Response {
    match e {
        LakeAdminError::InvalidAccessKey => invalid(
            "access_key",
            texts::tr!("api.machines.invalidLakeAccessKey"),
        ),
        LakeAdminError::InvalidSecret => {
            invalid("secret_key", texts::tr!("api.machines.invalidLakeSecret"))
        }
        LakeAdminError::Unavailable => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_lake_keys",
            texts::tr!("api.machines.lakeKeysUnavailable"),
        )
        .into_response(),
        LakeAdminError::Backend(e) => {
            ApiError::internal(format!("lake admin keys: {e}")).into_response()
        }
    }
}
