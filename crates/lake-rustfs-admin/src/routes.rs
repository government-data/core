//! `lake-admin/1`'s routes (decision 0078): the caller check, `GET /about`,
//! `POST /policies/session` and the error shape. The operations on RustFS
//! come with steps 11 and 12; until then they answer `404 unknown`.

use axum::body::Body;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Request};
use axum::http::{Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use contracts::lake_admin::{self as wire, ErrorBody, ErrorCode};
use contracts::LakeGrant;

use crate::policy;
use crate::tls::Peer;

/// The only role whose certificate is answered: the core server's (0066).
pub const CALLER_ROLE: &str = "app";

/// The implementation's name, as `GET /about` gives it.
pub const IMPLEMENTATION: &str = "lake-rustfs";

/// Every route of the adapter, behind the caller check.
pub fn router() -> Router {
    Router::new()
        .route(&wire::paths::about(), get(about))
        .route(&wire::paths::session_policy(), post(session_policy))
        .fallback(unserved)
        .layer(middleware::from_fn(caller_check))
}

async fn about() -> Json<wire::About> {
    Json(wire::About {
        protocols: vec![wire::PROTOCOL.into()],
        implementation: IMPLEMENTATION.into(),
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

/// A grant in RustFS's dialect; changes nothing. The policy is answered,
/// never logged.
async fn session_policy(body: Bytes) -> Result<Json<wire::SessionPolicy>, Error> {
    let what = "POST policies/session";
    let grant: LakeGrant = serde_json::from_slice(&body)
        .map_err(|e| Error::new(ErrorCode::Invalid, format!("{what}: not a grant: {e}")))?;
    let policy = policy::session_policy(&grant).map_err(|e| {
        let message = format!("{what} on {}: {}", grant.warehouse, e.0);
        Error::new(ErrorCode::Invalid, message)
    })?;
    Ok(Json(wire::SessionPolicy { policy }))
}

async fn unserved(method: Method, request: Request<Body>) -> Error {
    let path = request.uri().path();
    Error::new(ErrorCode::Unknown, format!("{method} {path}: not served"))
}

/// Refuses any caller but the core server, then logs what was answered:
/// the operation, its status and code, never a body.
async fn caller_check(
    ConnectInfo(peer): ConnectInfo<Peer>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let operation = format!("{} {}", request.method(), request.uri().path());
    let response = if peer.role.as_deref() == Some(CALLER_ROLE) {
        next.run(request).await
    } else {
        let message = format!("{operation}: not the core server's certificate");
        Error::new(ErrorCode::Refused, message).into_response()
    };
    let status = response.status().as_u16();
    match code_of(status) {
        Some(code) => {
            tracing::warn!(%operation, status, code = ?code, role = ?peer.role, "answered")
        }
        None => tracing::info!(%operation, status, "answered"),
    }
    response
}

fn code_of(status: u16) -> Option<ErrorCode> {
    [
        ErrorCode::Invalid,
        ErrorCode::Refused,
        ErrorCode::Unknown,
        ErrorCode::Conflict,
        ErrorCode::Lake,
        ErrorCode::Unavailable,
    ]
    .into_iter()
    .find(|c| c.status() == status)
}

/// A non-2xx answer in the protocol's shape. Its message names the
/// operation and the warehouse or pool, never a key or a policy.
#[derive(Debug)]
pub struct Error(ErrorBody);

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self(ErrorBody::new(code, message))
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.code.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(self.0)).into_response()
    }
}
