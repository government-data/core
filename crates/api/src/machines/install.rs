//! What a new machine is installed with (decision 0029; 0066 step 11), on
//! the public entry point and without a session:
//!
//! | Route                               | For                                 |
//! | ----------------------------------- | ----------------------------------- |
//! | `GET /install/join.sh`              | anyone: the command checks its hash |
//! | `GET /install/ca.crt`               | anyone: the script checks its hash  |
//! | `GET /install/{role}/compose.yml`   | a usable join token of that role    |
//! | `GET /install/{role}/files/{name}`  | the same: a file it names           |
//! | `GET /install/{role}/root-keys.env` | the same: a lake's root keys        |
//!
//! The compose file carries the installation's details, so it and its
//! entrypoints are given only
//! against a join token (`Authorization: Bearer qdj_…`, never in the URL),
//! checked as unused and not expired and **not** used up: the node agent
//! joins with it afterwards. The routes that check a token are limited per
//! address by a limit of their own (`INSTALL_RATE_LIMIT_*`), not joining's. Neither the token nor anything of it is logged or answered. A lake's
//! root keys are answered only there, audited, with no hash and `no-store`.

use std::sync::Arc;
use std::time::SystemTime;

use application::{InstallFileError, JoinCommand, MachinesService};
use auth_gateway::{InstallFile, MachineRole};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use http_kit::rate_limit::{RateLimitConfig, RateLimiter};
use serde::Serialize;
use utoipa::ToSchema;

use super::join::too_many;
use super::{no_ca, origin};
use crate::auth::ClientIp;
use crate::error::ApiError;

/// The install routes' state: the use cases and the limit per address of
/// the route that checks a token.
#[derive(Debug, Clone)]
pub struct InstallState {
    machines: MachinesService,
    rate_limit: Arc<RateLimiter>,
}

/// Mount the install routes; `rate_limit` is per client IP, for the routes
/// that check a token.
pub fn routes(machines: MachinesService, rate_limit: RateLimitConfig) -> Router {
    Router::new()
        .route("/install/join.sh", get(join_script_handler))
        .route("/install/ca.crt", get(install_ca_handler))
        .route("/install/{role}/compose.yml", get(role_compose_handler))
        .route("/install/{role}/files/{name}", get(role_file_handler))
        .route("/install/{role}/root-keys.env", get(role_root_keys_handler))
        .route("/install/{role}/pool.env", get(role_pool_env_handler))
        .with_state(InstallState {
            machines,
            rate_limit: Arc::new(RateLimiter::new(rate_limit)),
        })
}

/// The command Settings shows to add the machine, with every hash in it.
#[derive(Debug, Serialize, ToSchema)]
pub struct JoinCommandOut {
    /// Where the command downloads `join.sh` from.
    script_url: String,
    /// The SHA-256 of `join.sh`, which the command checks before running it.
    script_sha256: String,
    /// The CA's SHA-256 (of its DER), which the script checks: `--ca-sha256`.
    ca_sha256: String,
    /// The command to run on the new server, as root; it holds the token.
    command: String,
}

impl From<JoinCommand> for JoinCommandOut {
    fn from(c: JoinCommand) -> Self {
        Self {
            script_url: c.script_url,
            script_sha256: c.script_sha256,
            ca_sha256: c.ca_sha256,
            command: c.command,
        }
    }
}

/// `GET /install/join.sh`: the script the join command runs once its
/// SHA-256 is checked.
#[utoipa::path(
    get,
    path = "/install/join.sh",
    responses(
        (status = 200, description = "join.sh; its SHA-256 in X-Content-SHA256", body = String, content_type = "text/plain"),
        (status = 503, description = "This API serves no installation files"),
    ),
    tag = "machines",
)]
pub async fn join_script_handler(State(state): State<InstallState>) -> Response {
    match state.machines.join_script() {
        Some(script) => served(script, "text/x-shellscript; charset=utf-8", false),
        None => unavailable(),
    }
}

/// `GET /install/ca.crt`: the installation's CA, which `join.sh` checks
/// against `--ca-sha256` before it sends the token anywhere.
#[utoipa::path(
    get,
    path = "/install/ca.crt",
    responses(
        (status = 200, description = "The CA, PEM; its SHA-256 (of its DER) in X-Content-SHA256", body = String, content_type = "application/x-pem-file"),
        (status = 503, description = "The installation has no CA"),
    ),
    tag = "machines",
)]
pub async fn install_ca_handler(State(state): State<InstallState>) -> Response {
    match state.machines.install_ca() {
        Some(ca) => served(ca, "application/x-pem-file", false),
        None => no_ca(),
    }
}

/// `GET /install/{role}/compose.yml`: the compose file a machine of `role`
/// runs, for the join token in `Authorization: Bearer`, which is checked
/// and not used up.
#[utoipa::path(
    get,
    path = "/install/{role}/compose.yml",
    params(("role" = String, Path, description = "The machine's role, e.g. worker")),
    responses(
        (status = 200, description = "The compose file; its SHA-256 in X-Content-SHA256", body = String, content_type = "application/yaml"),
        (status = 401, description = "No join token, or one unknown, used, expired or of another role"),
        (status = 404, description = "No compose file for that role yet"),
        (status = 429, description = "Too many attempts from this address"),
        (status = 503, description = "This API serves no installation files"),
    ),
    tag = "machines",
)]
pub async fn role_compose_handler(
    State(state): State<InstallState>,
    ClientIp(ip): ClientIp,
    Path(role): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(limited) = limited(&state, &ip.to_string()) {
        return limited;
    }
    let token = bearer(&headers).unwrap_or_default();
    let Some(role) = MachineRole::parse(&role) else {
        return not_found();
    };
    let compose = state.machines.role_compose(token, role).await;
    answer(compose, "application/yaml; charset=utf-8")
}

/// `GET /install/{role}/files/{name}`: a file the compose file of `role`
/// names (an entrypoint, or the role's Alloy configuration), on the same
/// terms as the compose file; its SHA-256 is named in `join.sh`.
#[utoipa::path(
    get,
    path = "/install/{role}/files/{name}",
    params(
        ("role" = String, Path, description = "The machine's role, e.g. lake-replica"),
        ("name" = String, Path, description = "The file's name, e.g. rustfs-tls.sh or worker.alloy"),
    ),
    responses(
        (status = 200, description = "The file, an entrypoint (`.sh`) or an Alloy configuration (`.alloy`, text/plain); its SHA-256 in X-Content-SHA256", content(
            (String = "text/x-shellscript"),
            (String = "text/plain"),
        )),
        (status = 401, description = "No join token, or one unknown, used, expired or of another role"),
        (status = 404, description = "No such file for that role"),
        (status = 429, description = "Too many attempts from this address"),
        (status = 503, description = "This API serves no installation files"),
    ),
    tag = "machines",
)]
pub async fn role_file_handler(
    State(state): State<InstallState>,
    ClientIp(ip): ClientIp,
    Path((role, name)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Some(limited) = limited(&state, &ip.to_string()) {
        return limited;
    }
    let token = bearer(&headers).unwrap_or_default();
    let Some(role) = MachineRole::parse(&role) else {
        return not_found();
    };
    let file = state.machines.role_file(token, role, &name).await;
    let content_type = if name.ends_with(".sh") {
        "text/x-shellscript; charset=utf-8"
    } else {
        "text/plain; charset=utf-8"
    };
    answer(file, content_type)
}

/// `GET /install/{role}/root-keys.env`: RustFS's root keys of the lake a
/// node of `role` belongs to, made by the application (0066 step 11), on
/// the compose file's terms; audited every time, never the keys.
#[utoipa::path(
    get,
    path = "/install/{role}/root-keys.env",
    params(("role" = String, Path, description = "A lake role: lake-replica")),
    responses(
        (status = 200, description = "RUSTFS_ACCESS_KEY and RUSTFS_SECRET_KEY, an environment file", body = String, content_type = "text/plain"),
        (status = 401, description = "No join token, or one unknown, used, expired or of another role"),
        (status = 404, description = "No root keys for that role"),
        (status = 429, description = "Too many attempts from this address"),
        (status = 503, description = "This API serves no installation files"),
    ),
    tag = "machines",
)]
pub async fn role_root_keys_handler(
    State(state): State<InstallState>,
    ClientIp(ip): ClientIp,
    Path(role): Path<String>,
    headers: HeaderMap,
) -> Response {
    let ip = ip.to_string();
    if let Some(limited) = limited(&state, &ip) {
        return limited;
    }
    let token = bearer(&headers).unwrap_or_default();
    let Some(role) = MachineRole::parse(&role) else {
        return not_found();
    };
    let from = origin(ip, &headers);
    match state.machines.role_root_keys(token, role, &from).await {
        Ok(body) => (
            [
                (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            body,
        )
            .into_response(),
        Err(e) => answer(Err(e), "text/plain"),
    }
}

/// `GET /install/{role}/pool.env`: a lake pool node's layout (0066 step
/// 11, rulings 1 and 4), for a usable join token of the role `lake` that
/// names the node; the token is not used up.
#[utoipa::path(
    get,
    path = "/install/{role}/pool.env",
    params(
        ("role" = String, Path, description = "lake"),
        ("Authorization" = String, Header, description = "Bearer and the join token"),
    ),
    responses(
        (status = 200, description = "QD_POOL, QD_POOL_NODE, QD_POOL_NODES, QD_POOL_DRIVES, RUSTFS_VOLUMES and, when known, QD_LAKE_REPLICA_GATE", content_type = "text/plain"),
        (status = 401, description = "No join token, or one unknown, used, expired or of another role"),
        (status = 404, description = "No pool for that token"),
        (status = 429, description = "Too many attempts from this address"),
        (status = 503, description = "This API serves no installation files"),
    ),
    tag = "machines",
)]
pub async fn role_pool_env_handler(
    State(state): State<InstallState>,
    ClientIp(ip): ClientIp,
    Path(role): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(limited) = limited(&state, &ip.to_string()) {
        return limited;
    }
    let token = bearer(&headers).unwrap_or_default();
    let Some(role) = MachineRole::parse(&role) else {
        return not_found();
    };
    match state.machines.role_pool_env(token, role).await {
        Ok(body) => (
            [
                (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            body,
        )
            .into_response(),
        Err(e) => answer(Err(e), "text/plain"),
    }
}

/// The answer of the routes that check a token, if this address asked too
/// often.
fn limited(state: &InstallState, ip: &str) -> Option<Response> {
    let exceeded = state
        .rate_limit
        .check(&format!("install:{ip}"), SystemTime::now())
        .err()?;
    tracing::warn!(%ip, "install rate limit exceeded");
    Some(too_many(exceeded.retry_after.as_secs()))
}

/// A file given against a token, or why not.
fn answer(file: Result<InstallFile, InstallFileError>, content_type: &'static str) -> Response {
    match file {
        Ok(file) => served(file, content_type, true),
        Err(InstallFileError::Refused) => refused(),
        Err(InstallFileError::NotFound) => not_found(),
        Err(InstallFileError::Unavailable) => unavailable(),
        Err(InstallFileError::Backend(e)) => {
            ApiError::internal(format!("install: {e}")).into_response()
        }
    }
}

/// The token of `Authorization: Bearer …`, if any.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

/// A file with its SHA-256; `private` for one given against a token, which
/// no cache may keep.
fn served(file: InstallFile, content_type: &'static str, private: bool) -> Response {
    let cache = if private { "no-store" } else { "no-cache" };
    let mut response = (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, cache),
        ],
        file.body,
    )
        .into_response();
    if let Ok(sha) = HeaderValue::from_str(&file.sha256) {
        response.headers_mut().insert("x-content-sha256", sha);
    }
    response
}

fn refused() -> Response {
    let message = texts::tr!("api.machines.joinRefused");
    ApiError::new(StatusCode::UNAUTHORIZED, "join_refused", message).into_response()
}

fn not_found() -> Response {
    ApiError::not_found(texts::tr!("api.machines.noInstallFiles")).into_response()
}

fn unavailable() -> Response {
    let message = texts::tr!("api.machines.installUnavailable");
    ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "no_install", message).into_response()
}
