//! The lake's gate (0066, point 10): the Traefik in front of RustFS, on
//! each lake node, requires the client certificate and asks
//! `GET /internal/machines/lake/check` through `forwardAuth`, on the
//! application's entry point of the gates' own. Two certificates are in a
//! check. The gate's own, which it presents to ask and the application's
//! Traefik writes in the header every machine route reads
//! (`passTLSClientCert`): who asks, which no one can forge. And the one of
//! the machine at the gate's door, which the gate's Traefik names in
//! `X-Forwarded-Tls-Client-Cert-Info`: for whom, believed only because a
//! gate asks. 200 with an empty body lets the request through; anything
//! else is Traefik's answer to it. The original method and path
//! (`X-Forwarded-Method`, `X-Forwarded-Uri`) are not read: the answer is
//! the machine's. No session, no CSRF. A pass is not audited, one per
//! request to the lake; a refusal is. Neither the request nor a
//! certificate is logged.

use application::{LakeCheckError, MachinesService};
use auth_gateway::http::client_cert::{
    certificate_refused, ClientCertificateError, ClientCertificates,
};
use auth_gateway::http::client_cert_info::named_machine;
use auth_gateway::MachineCertificate;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Router};

use super::origin;
use crate::auth::ClientIp;
use crate::error::ApiError;

/// The path, as the API sees it.
pub const CHECK_PATH: &str = "/internal/machines/lake/check";

/// Mount the route; `certificates` says whether the client certificate
/// header is trusted.
pub fn routes(machines: MachinesService, certificates: ClientCertificates) -> Router {
    Router::new()
        .route(CHECK_PATH, get(lake_check_handler))
        .with_state(machines)
        .layer(Extension(certificates))
}

/// `GET /internal/machines/lake/check`: may the machine this gate names
/// reach the lake?
#[utoipa::path(
    get,
    path = "/internal/machines/lake/check",
    responses(
        (status = 200, description = "It may: asked by a gate of the lake (an approved or draining machine of role lake or lake-replica, by its certificate), for an approved or draining machine of any role, named in X-Forwarded-Tls-Client-Cert-Info with its serial in use or next. Kept for 5 seconds"),
        (status = 401, description = "Not asked by a gate, or for no such machine"),
        (status = 503, description = "The client certificate header is not trusted"),
    ),
    tag = "machines",
)]
pub async fn lake_check_handler(
    State(machines): State<MachinesService>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    asker: Result<MachineCertificate, ClientCertificateError>,
) -> Response {
    let asker = match asker {
        Ok(asker) => asker,
        Err(refused) => return refused.into_response(),
    };
    let (named, origin) = (named_machine(&headers), origin(ip, &headers));
    let checked = machines.check_lake_for(&asker, named.as_ref(), &origin);
    match checked.await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(LakeCheckError::Refused) => certificate_refused().into_response(),
        Err(LakeCheckError::Backend(e)) => {
            ApiError::internal(format!("lake check: {e}")).into_response()
        }
    }
}
