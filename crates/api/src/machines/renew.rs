//! `POST /internal/machines/renew` (0066, steps 3 and 4): a machine renews
//! its certificate with the one it has. Served only on Traefik's internal
//! entry point, which requires a certificate of the installation's CA and
//! passes it in `X-Forwarded-Tls-Client-Cert`; the public one does not route
//! here. The certificate is the proof: no session, no CSRF, no limit per
//! address.
//!
//! The header is read only with `INTERNAL_CLIENT_CERT_HEADER=trusted`
//! ([`ClientCertificates`]); without it, 503. Neither the request nor the
//! certificate is logged.

use application::{MachinesService, RenewError};
use auth_gateway::http::client_cert::{
    certificate_refused, ClientCertificateError, ClientCertificates,
};
use auth_gateway::MachineCertificate;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::join::JOIN_BODY_LIMIT;
use super::{epoch, invalid, origin};
use crate::auth::ClientIp;
use crate::error::ApiError;

/// The path, as the API sees it (Traefik strips `/api`).
pub const RENEW_PATH: &str = "/internal/machines/renew";

/// Mount the renew route; `certificates` says whether the client
/// certificate header is trusted.
pub fn routes(machines: MachinesService, certificates: ClientCertificates) -> Router {
    Router::new()
        .route(RENEW_PATH, post(renew_handler))
        .with_state(machines)
        .layer(Extension(certificates))
        .layer(DefaultBodyLimit::max(JOIN_BODY_LIMIT))
}

/// A machine asks for a new certificate.
#[derive(Deserialize, ToSchema)]
pub struct RenewIn {
    /// The PKCS#10 signing request of a new EC P-256 key, PEM. Only its key
    /// is used: subject, names and lifetime are the CA's.
    csr: String,
}

impl std::fmt::Debug for RenewIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenewIn").finish_non_exhaustive()
    }
}

/// The machine's new certificate.
#[derive(Debug, Serialize, ToSchema)]
pub struct RenewOut {
    /// Its certificate, PEM: the same `O=qd, OU=<role>, CN=<name>`.
    certificate: String,
    /// The installation's CA, PEM.
    ca: String,
    /// The new certificate's serial, lowercase hex. Until the machine first
    /// presents it, the one it renewed with is accepted too.
    serial: String,
    /// When it ends, in Unix epoch seconds: a day, or a year for Trino's
    /// coordinator.
    not_after: i64,
    /// The machine's state: `pending`, `approved` or `draining`.
    state: String,
}

/// `POST /internal/machines/renew`: a new certificate for the machine of
/// the certificate presented.
#[utoipa::path(
    post,
    path = "/internal/machines/renew",
    request_body = RenewIn,
    responses(
        (status = 200, description = "Its new certificate", body = RenewOut),
        (status = 401, description = "No certificate, or one unknown, replaced, of a retired machine or of another role"),
        (status = 413, description = "The body is too large"),
        (status = 422, description = "A bad signing request"),
        (status = 503, description = "No CA, or the client certificate header is not trusted here"),
    ),
    tag = "machines",
)]
pub async fn renew_handler(
    State(machines): State<MachinesService>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    presented: Result<MachineCertificate, ClientCertificateError>,
    Json(body): Json<RenewIn>,
) -> Response {
    let origin = origin(ip, &headers);
    let presented = match presented {
        Ok(presented) => presented,
        Err(ClientCertificateError::NotTrusted) => {
            return ClientCertificateError::NotTrusted.into_response()
        }
        Err(refused) => {
            machines.renew_without_certificate(&origin).await;
            return refused.into_response();
        }
    };
    match machines.renew(&presented, &body.csr, &origin).await {
        Ok(r) => Json(RenewOut {
            certificate: r.certificate,
            ca: r.ca,
            serial: r.serial,
            not_after: epoch(r.not_after),
            state: r.machine.state.as_str().to_string(),
        })
        .into_response(),
        Err(e) => error(e),
    }
}

fn error(e: RenewError) -> Response {
    match e {
        RenewError::Refused => certificate_refused().into_response(),
        RenewError::InvalidRequest => invalid("csr", texts::tr!("api.machines.invalidRequest")),
        RenewError::Unavailable => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_ca",
            texts::tr!("api.machines.renewNoCa"),
        )
        .into_response(),
        RenewError::Backend(e) => ApiError::internal(format!("renew: {e}")).into_response(),
    }
}
