//! A machine's way through Settings → Execution (decisions 0029 and 0066):
//! pending → approved → draining → retired, for whoever may do
//! `admin:ManageMachines`; the rules are [`MachinesService`]'s.

use application::{MachinesError, MachinesService, Origin};
use auth_gateway::{Machine, MachineId};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use domain::AdminId;

use super::{error, origin, MachineOut};
use crate::auth::ClientIp;
use crate::session::CurrentSession;

/// `POST /machines/{id}/approve`: a pending machine starts taking work.
#[utoipa::path(
    post,
    path = "/machines/{id}/approve",
    params(("id" = String, Path, description = "The machine's id")),
    responses(
        (status = 200, description = "Approved", body = MachineOut),
        (status = 404, description = "No such machine"),
        (status = 409, description = "It is not pending"),
    ),
    tag = "machines",
)]
pub async fn approve_handler(
    State(machines): State<MachinesService>,
    Extension(session): Extension<CurrentSession>,
    Path(id): Path<String>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
) -> Response {
    let (id, by, origin) = who(id, &session, ip, &headers);
    answer(machines.approve(&id, &by, &origin).await)
}

/// `POST /machines/{id}/drain`: an approved machine finishes what it has and
/// gets nothing new.
#[utoipa::path(
    post,
    path = "/machines/{id}/drain",
    params(("id" = String, Path, description = "The machine's id")),
    responses(
        (status = 200, description = "Draining", body = MachineOut),
        (status = 404, description = "No such machine"),
        (status = 409, description = "It is not approved"),
    ),
    tag = "machines",
)]
pub async fn drain_handler(
    State(machines): State<MachinesService>,
    Extension(session): Extension<CurrentSession>,
    Path(id): Path<String>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
) -> Response {
    let (id, by, origin) = who(id, &session, ip, &headers);
    answer(machines.drain(&id, &by, &origin).await)
}

/// `POST /machines/{id}/retire`: the machine is out for good; its
/// certificate is refused from now on.
#[utoipa::path(
    post,
    path = "/machines/{id}/retire",
    params(("id" = String, Path, description = "The machine's id")),
    responses(
        (status = 200, description = "Retired", body = MachineOut),
        (status = 404, description = "No such machine"),
        (status = 409, description = "It is retired already"),
    ),
    tag = "machines",
)]
pub async fn retire_handler(
    State(machines): State<MachinesService>,
    Extension(session): Extension<CurrentSession>,
    Path(id): Path<String>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
) -> Response {
    let (id, by, origin) = who(id, &session, ip, &headers);
    answer(machines.retire(&id, &by, &origin).await)
}

fn who(
    id: String,
    session: &CurrentSession,
    ip: String,
    headers: &HeaderMap,
) -> (MachineId, AdminId, Origin) {
    let by = AdminId::new(&session.admin_id);
    (MachineId::new(id), by, origin(ip, headers))
}

fn answer(result: Result<Machine, MachinesError>) -> Response {
    match result {
        Ok(machine) => Json(MachineOut::from(machine)).into_response(),
        Err(e) => error(e),
    }
}
