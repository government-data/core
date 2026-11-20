//! Trino's coordinator restarts only when Trino is free (0066, point 9):
//! its agent asks `POST /internal/machines/trino/hold` with the certificate
//! in use, and once Trino answers after the restart, ends the hold with
//! `POST /internal/machines/trino/release` and its new one. Served on
//! Traefik's internal entry point (the machines' router), as renewing is:
//! the certificate is the proof, no session, no CSRF. Neither the request
//! nor the certificate is logged.

use application::{MachinesService, TrinoHoldError};
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
pub const HOLD_PATH: &str = "/internal/machines/trino/hold";
pub const RELEASE_PATH: &str = "/internal/machines/trino/release";

/// Mount the two routes; `certificates` says whether the client
/// certificate header is trusted.
pub fn routes(machines: MachinesService, certificates: ClientCertificates) -> Router {
    Router::new()
        .route(HOLD_PATH, post(trino_hold_handler))
        .route(RELEASE_PATH, post(trino_release_handler))
        .with_state(machines)
        .layer(Extension(certificates))
}

/// Whether Trino is free, and if so its jobs held.
#[derive(Debug, Serialize, ToSchema)]
pub struct HoldOut {
    /// No job that uses Trino running and Trino running no query: the jobs
    /// that use it are held until released or 15 minutes. Otherwise nothing
    /// is held: ask again later.
    free: bool,
    /// The jobs that use Trino running now.
    running_jobs: u64,
    /// The queries Trino runs or queues now; `null` if it did not answer.
    running_queries: Option<u64>,
}

/// Whether a hold was ended.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReleaseOut {
    /// `false` when there was none in force (it had expired by itself).
    released: bool,
}

/// `POST /internal/machines/trino/hold`: may Trino's coordinator restart?
#[utoipa::path(
    post,
    path = "/internal/machines/trino/hold",
    responses(
        (status = 200, description = "Whether Trino is free; if so its jobs are held", body = HoldOut),
        (status = 401, description = "Not an approved or draining Trino coordinator's certificate"),
        (status = 503, description = "No Trino or queue here, or the client certificate header is not trusted"),
    ),
    tag = "machines",
)]
pub async fn trino_hold_handler(
    State(machines): State<MachinesService>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    presented: Result<MachineCertificate, ClientCertificateError>,
) -> Response {
    let presented = match presented {
        Ok(presented) => presented,
        Err(refused) => return refused.into_response(),
    };
    match machines.hold_trino(&presented, &origin(ip, &headers)).await {
        Ok(hold) => Json(HoldOut {
            free: hold.free,
            running_jobs: hold.running_jobs,
            running_queries: hold.running_queries,
        })
        .into_response(),
        Err(e) => error(e),
    }
}

/// `POST /internal/machines/trino/release`: Trino answers again.
#[utoipa::path(
    post,
    path = "/internal/machines/trino/release",
    responses(
        (status = 200, description = "The hold ended", body = ReleaseOut),
        (status = 401, description = "Not an approved or draining Trino coordinator's certificate"),
        (status = 503, description = "No Trino or queue here, or the client certificate header is not trusted"),
    ),
    tag = "machines",
)]
pub async fn trino_release_handler(
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
        .release_trino(&presented, &origin(ip, &headers))
        .await
    {
        Ok(released) => Json(ReleaseOut { released }).into_response(),
        Err(e) => error(e),
    }
}

fn error(e: TrinoHoldError) -> Response {
    match e {
        TrinoHoldError::Refused => certificate_refused().into_response(),
        TrinoHoldError::Unavailable => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "trino_unavailable",
            texts::tr!("api.machines.trinoUnavailable"),
        )
        .into_response(),
        TrinoHoldError::Backend(e) => {
            ApiError::internal(format!("trino hold: {e}")).into_response()
        }
    }
}
