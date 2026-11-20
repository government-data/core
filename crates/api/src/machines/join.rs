//! `POST /install/join` (0029, 0066 step 3): a machine with no certificate
//! and no account yet joins with its join token, which is its only proof,
//! and the signing request of the key it made. No session; limited per IP
//! as signing in is, and its body to [`JOIN_BODY_LIMIT`].
//!
//! How long the certificate lasts is the role's (`MachineRole::
//! certificate_term`), never the machine's to ask: the body has no lifetime.
//! An unknown, used or expired token gets one answer, the same for the
//! three. Neither the token nor the request is logged.

use std::sync::Arc;
use std::time::SystemTime;

use application::{JoinError, MachinesService};
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use http_kit::rate_limit::{RateLimitConfig, RateLimiter};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{epoch, invalid, no_ca, origin};
use crate::auth::ClientIp;
use crate::error::ApiError;

/// The most a join's body may hold: a P-256 request is under 1 KB, and the
/// token and the name are short.
pub const JOIN_BODY_LIMIT: usize = 8 * 1024;

/// The join route's state: the use cases and the limit per address.
#[derive(Debug, Clone)]
pub struct JoinState {
    machines: MachinesService,
    rate_limit: Arc<RateLimiter>,
}

/// Mount the join route; `rate_limit` is per client IP.
pub fn routes(machines: MachinesService, rate_limit: RateLimitConfig) -> Router {
    Router::new()
        .route("/install/join", post(join_handler))
        .with_state(JoinState {
            machines,
            rate_limit: Arc::new(RateLimiter::new(rate_limit)),
        })
        .layer(DefaultBodyLimit::max(JOIN_BODY_LIMIT))
}

/// A machine asks to join.
#[derive(Deserialize, ToSchema)]
pub struct JoinIn {
    /// The join token from Settings → Execution (`qdj_…`).
    token: String,
    /// The machine's name, its certificate's `CN`: lowercase letters,
    /// digits and hyphens.
    #[schema(example = "worker-01")]
    name: String,
    /// The PKCS#10 signing request of its EC P-256 key, PEM. Only its key
    /// is used: subject, names and lifetime are the CA's.
    csr: String,
}

impl std::fmt::Debug for JoinIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinIn")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// The machine's first certificate.
#[derive(Debug, Serialize, ToSchema)]
pub struct JoinOut {
    /// Its certificate, PEM: `O=qd, OU=<role>, CN=<name>`.
    certificate: String,
    /// The installation's CA, PEM.
    ca: String,
    /// The certificate's serial, lowercase hex.
    serial: String,
    /// When it ends, in Unix epoch seconds: a day, or a year for a role
    /// that takes its certificate only at a restart (Trino's coordinator).
    not_after: i64,
    /// `pending`: it gets no work until a person approves it.
    state: String,
}

/// `POST /install/join`: exchange a join token and a signing request for
/// the machine's certificate.
#[utoipa::path(
    post,
    path = "/install/join",
    request_body = JoinIn,
    responses(
        (status = 201, description = "Joined, pending; its certificate", body = JoinOut),
        (status = 401, description = "The token is unknown, used or expired"),
        (status = 409, description = "Another machine has that name, or the token is another node's"),
        (status = 413, description = "The body is too large"),
        (status = 422, description = "A bad name or signing request; the token is not used"),
        (status = 429, description = "Too many attempts from this address"),
        (status = 503, description = "The installation has no CA"),
    ),
    tag = "machines",
)]
pub async fn join_handler(
    State(state): State<JoinState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(body): Json<JoinIn>,
) -> Response {
    if let Err(exceeded) = state
        .rate_limit
        .check(&format!("join:{ip}"), SystemTime::now())
    {
        tracing::warn!(%ip, "join rate limit exceeded");
        return too_many(exceeded.retry_after.as_secs());
    }
    let joined = state
        .machines
        .join(&body.token, &body.name, &body.csr, &origin(ip, &headers))
        .await;
    match joined {
        Ok(j) => (
            StatusCode::CREATED,
            Json(JoinOut {
                certificate: j.certificate,
                ca: j.ca,
                serial: j.serial,
                not_after: epoch(j.not_after),
                state: j.machine.state.as_str().to_string(),
            }),
        )
            .into_response(),
        Err(e) => error(e),
    }
}

pub(super) fn too_many(retry_after_secs: u64) -> Response {
    let message = texts::tr!("api.machines.tooManyAttempts");
    let mut response =
        ApiError::new(StatusCode::TOO_MANY_REQUESTS, "too_many_attempts", message).into_response();
    if let Ok(value) = HeaderValue::from_str(&retry_after_secs.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

fn error(e: JoinError) -> Response {
    match e {
        JoinError::Refused => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "join_refused",
            texts::tr!("api.machines.joinRefused"),
        )
        .into_response(),
        JoinError::NameTaken => {
            ApiError::conflict("name_taken", texts::tr!("api.machines.nameTaken")).into_response()
        }
        JoinError::NodeName => {
            ApiError::conflict("node_name", texts::tr!("api.machines.nodeName")).into_response()
        }
        JoinError::InvalidName => invalid("name", texts::tr!("api.machines.invalidName")),
        JoinError::InvalidRequest => invalid("csr", texts::tr!("api.machines.invalidRequest")),
        JoinError::Unavailable => no_ca(),
        JoinError::Backend(e) => ApiError::internal(format!("join: {e}")).into_response(),
    }
}
