//! Dolt restarts to take a renewed certificate only when it is free (0066,
//! point 11): the application VM's agent asks `POST
//! /internal/machines/dolt/hold` with its certificate (role `app`), and
//! once Dolt answers after the restart, ends the hold with `POST
//! /internal/machines/dolt/release`. Served on
//! Traefik's internal entry point (the machines' router), as renewing is:
//! the certificate is the proof, no session, no CSRF; allowed while the
//! license is read-only (`read_only.rs`). Neither the request nor the
//! certificate is logged.

use application::{DoltHoldError, MachinesService};
use auth_gateway::http::client_cert::{
    certificate_refused, ClientCertificateError, ClientCertificates,
};
use auth_gateway::MachineCertificate;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use serde::Serialize;
use utoipa::ToSchema;

use super::origin;
use crate::auth::ClientIp;
use crate::error::ApiError;

/// The paths, as the API sees them (Traefik strips `/api`).
pub const HOLD_PATH: &str = "/internal/machines/dolt/hold";
pub const RELEASE_PATH: &str = "/internal/machines/dolt/release";

/// Mount the two routes; `certificates` says whether the client
/// certificate header is trusted.
pub fn routes(machines: MachinesService, certificates: ClientCertificates) -> Router {
    Router::new()
        .route(HOLD_PATH, post(dolt_hold_handler))
        .route(RELEASE_PATH, post(dolt_release_handler))
        .with_state(machines)
        .layer(Extension(certificates))
}

/// Whether Dolt is free, and if so the catalog held.
#[derive(Debug, Serialize, ToSchema)]
pub struct DoltHoldOut {
    /// No operation of the catalog in flight in the API and no statement
    /// running in Dolt: the catalog is held until released or 5 minutes.
    /// Otherwise nothing is held: ask again later.
    free: bool,
    /// The catalog's operations in flight in the API now.
    in_flight: u64,
    /// The other connections running a statement in Dolt; `null` if it was
    /// not asked or did not answer.
    running: Option<u64>,
}

/// Whether a hold was ended.
#[derive(Debug, Serialize, ToSchema)]
pub struct DoltReleaseOut {
    /// `false` when there was none in force (it had expired by itself).
    released: bool,
}

/// `POST /internal/machines/dolt/hold`: may Dolt restart?
#[utoipa::path(
    post,
    path = "/internal/machines/dolt/hold",
    responses(
        (status = 200, description = "Whether Dolt is free; if so the catalog is held", body = DoltHoldOut),
        (status = 401, description = "Not an approved or draining app machine's certificate"),
        (status = 503, description = "No catalog here, or the client certificate header is not trusted"),
    ),
    tag = "machines",
)]
pub async fn dolt_hold_handler(
    State(machines): State<MachinesService>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    presented: Result<MachineCertificate, ClientCertificateError>,
) -> Response {
    let presented = match presented {
        Ok(presented) => presented,
        Err(refused) => return refused.into_response(),
    };
    match machines.hold_dolt(&presented, &origin(ip, &headers)).await {
        Ok(hold) => Json(DoltHoldOut {
            free: hold.free,
            in_flight: hold.in_flight,
            running: hold.running,
        })
        .into_response(),
        Err(e) => error(e),
    }
}

/// `POST /internal/machines/dolt/release`: Dolt answers again.
#[utoipa::path(
    post,
    path = "/internal/machines/dolt/release",
    responses(
        (status = 200, description = "The hold ended", body = DoltReleaseOut),
        (status = 401, description = "Not an approved or draining app machine's certificate"),
        (status = 503, description = "No catalog here, or the client certificate header is not trusted"),
    ),
    tag = "machines",
)]
pub async fn dolt_release_handler(
    State(machines): State<MachinesService>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    presented: Result<MachineCertificate, ClientCertificateError>,
) -> Response {
    let presented = match presented {
        Ok(presented) => presented,
        Err(refused) => return refused.into_response(),
    };
    match machines
        .release_dolt(&presented, &origin(ip, &headers))
        .await
    {
        Ok(released) => Json(DoltReleaseOut { released }).into_response(),
        Err(e) => error(e),
    }
}

fn error(e: DoltHoldError) -> Response {
    match e {
        DoltHoldError::Refused => certificate_refused().into_response(),
        DoltHoldError::Unavailable => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "dolt_unavailable",
            texts::tr!("api.machines.doltUnavailable"),
        )
        .into_response(),
        DoltHoldError::Backend(e) => ApiError::internal(format!("dolt hold: {e}")).into_response(),
    }
}
