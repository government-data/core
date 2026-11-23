//! The internal routes, for the host's update daemon
//! (the host's updater): the running version, what is left before it
//! can update, maintenance, what the administrator decided (`orders`) and the
//! daemon's state (`report`). And the administrator's entry (`admin-access`),
//! for the installer and the host's admin access command.
//!
//! They do not go through Traefik: they are served on their own port
//! (`QD_INTERNAL_ADDR`), which Compose publishes only on the host's
//! `127.0.0.1`, and every request carries the local token (`Authorization:
//! Bearer …`, `QD_INTERNAL_TOKEN_FILE`). The API never gets `docker.sock`: the
//! host is the one that updates.
//!
//! The token is not fixed: the updater rewrites it every day in the same file,
//! and the API reads it again as soon as it changes ([`InternalToken`]).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub use admin_access::AdminAccessRoute;

mod admin_access;

use application::{MaintenanceError, UpdatesError, UpdatesService};
use axum::{
    extract::{Query, Request, State},
    http::{header::AUTHORIZATION, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

/// The default horizon for "scheduled soon".
const DEFAULT_HORIZON: Duration = Duration::from_secs(30 * 60);

#[derive(Clone)]
struct Internal {
    updates: UpdatesService,
    access: Option<AdminAccessRoute>,
}

/// How long the previous token stays valid when the file changes: as long as
/// the requests that had already read it take.
const TOKEN_OVERLAP: Duration = Duration::from_secs(5 * 60);

/// The token every request has to carry: a fixed one (tests) or the one in a
/// file, read again if it changed.
pub struct InternalToken {
    file: Option<PathBuf>,
    state: Mutex<TokenState>,
}

struct TokenState {
    modified: Option<SystemTime>,
    current: String,
    previous: Option<(String, Instant)>,
}

impl std::fmt::Debug for InternalToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InternalToken")
            .field("file", &self.file)
            .finish_non_exhaustive()
    }
}

impl From<String> for InternalToken {
    fn from(token: String) -> Self {
        Self {
            file: None,
            state: Mutex::new(TokenState {
                modified: None,
                current: token.trim().to_string(),
                previous: None,
            }),
        }
    }
}

impl From<&str> for InternalToken {
    fn from(token: &str) -> Self {
        token.to_string().into()
    }
}

impl InternalToken {
    /// The minimum length of an internal token.
    pub const MIN_LEN: usize = 32;

    /// Read the token from `path`; an error if it is missing or too short.
    pub fn from_file(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let (modified, current) = read_token(&path)?;
        Ok(Self {
            file: Some(path),
            state: Mutex::new(TokenState {
                modified,
                current,
                previous: None,
            }),
        })
    }

    /// Whether `given` is the current token (or the previous one, while its
    /// overlap lasts). First, if the file changed, it reads it again.
    fn accepts(&self, given: &str) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(path) = &self.file {
            let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            if modified != state.modified {
                match read_token(path) {
                    Ok((modified, token)) => {
                        if token != state.current {
                            let old = std::mem::replace(&mut state.current, token);
                            state.previous = Some((old, Instant::now() + TOKEN_OVERLAP));
                        }
                        state.modified = modified;
                    }
                    // Half written or deleted: the one there was stays.
                    Err(e) => tracing::warn!("internal routes: {e}"),
                }
            }
        }
        if !state.current.is_empty() && same(given.as_bytes(), state.current.as_bytes()) {
            return true;
        }
        matches!(&state.previous, Some((old, until))
            if Instant::now() < *until && same(given.as_bytes(), old.as_bytes()))
    }
}

fn read_token(path: &std::path::Path) -> Result<(Option<SystemTime>, String), String> {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let token = std::fs::read_to_string(path)
        .map_err(|e| format!("QD_INTERNAL_TOKEN_FILE {}: {e}", path.display()))?
        .trim()
        .to_string();
    if token.len() < InternalToken::MIN_LEN {
        return Err(format!(
            "QD_INTERNAL_TOKEN_FILE {}: the token is too short",
            path.display()
        ));
    }
    Ok((modified, token))
}

/// The internal router. `token` is the one every request has to carry.
pub fn router(
    updates: UpdatesService,
    access: Option<AdminAccessRoute>,
    token: InternalToken,
) -> Router {
    let token = Arc::new(token);
    Router::new()
        .route("/internal/version", get(version))
        .route("/internal/drain", get(drain))
        .route("/internal/maintenance", post(maintenance))
        .route("/internal/orders", get(orders))
        .route("/internal/report", post(report))
        .route("/internal/admin-access", post(admin_access::admin_access))
        .with_state(Internal { updates, access })
        .layer(axum::middleware::from_fn(crate::language::per_request))
        .layer(axum::middleware::from_fn(move |req, next| {
            authorize(token.clone(), req, next)
        }))
}

async fn authorize(token: Arc<InternalToken>, req: Request, next: Next) -> Response {
    let given = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    if !token.accepts(given) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(req).await
}

/// Compare without the time telling how much matched.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn version() -> Json<serde_json::Value> {
    // `token_rotates`: the updater may rewrite the token (an older API reads it
    // only at start-up).
    Json(json!({ "version": domain::VERSION, "token_rotates": true }))
}

#[derive(Debug, Deserialize)]
struct DrainQuery {
    /// "Scheduled soon": within how many seconds.
    horizon_secs: Option<u64>,
}

/// What is still running, and whether it can update now without cutting anything.
async fn drain(State(s): State<Internal>, Query(q): Query<DrainQuery>) -> Response {
    let horizon = q.horizon_secs.map_or(DEFAULT_HORIZON, Duration::from_secs);
    let status = s.updates.drain_status(horizon).await;
    let maintenance = match s.updates.active().await {
        Ok(m) => m,
        Err(e) => return unavailable(&e),
    };
    Json(json!({
        "running": status.running,
        "deploying": status.deploying,
        "queued": status.queued,
        "scheduled_soon": status.scheduled_soon,
        "not_answering": &status.not_answering,
        "drained": status.is_drained(),
        "maintenance": maintenance,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct MaintenanceBody {
    active: bool,
    #[serde(default)]
    reason: String,
    /// How long it lasts if nobody ends it (renewed by asking again).
    #[serde(default)]
    secs: u64,
}

async fn maintenance(State(s): State<Internal>, Json(body): Json<MaintenanceBody>) -> Response {
    if !body.active {
        return match s.updates.end().await {
            Ok(()) => Json(json!({ "maintenance": null })).into_response(),
            Err(e) => unavailable(&e),
        };
    }
    match s
        .updates
        .begin(&body.reason, Duration::from_secs(body.secs))
        .await
    {
        Ok(m) => Json(json!({ "maintenance": m })).into_response(),
        Err(MaintenanceError::Unavailable(e)) => unavailable(&e),
        Err(MaintenanceError::Invalid(e)) => {
            (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response()
        }
    }
}

/// The policy and the pending "Update now".
async fn orders(State(s): State<Internal>) -> Response {
    match s.updates.orders().await {
        Ok(orders) => Json(orders).into_response(),
        Err(e) => unavailable(&e),
    }
}

/// The daemon's state: stored for the page, and what is new is notified.
async fn report(State(s): State<Internal>, Json(body): Json<serde_json::Value>) -> Response {
    match s.updates.report(body).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(UpdatesError::Invalid(field, e)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("{field}: {e}") })),
        )
            .into_response(),
        Err(e) => unavailable(&e.to_string()),
    }
}

fn unavailable(e: &str) -> Response {
    tracing::error!("internal routes: {e}");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": texts::tr!("api.internal.unavailable") })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_must_match_exactly() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"abcd"));
        assert!(!same(b"", b"abc"));
    }

    #[test]
    fn a_rewritten_token_file_is_read_again_and_the_old_one_lingers() {
        let dir = std::env::temp_dir().join(format!("orq-internal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("internal_token");
        let (old, new) = ("a".repeat(40), "b".repeat(40));
        std::fs::write(&path, &old).unwrap();
        let token = InternalToken::from_file(&path).unwrap();
        assert!(token.accepts(&old));
        assert!(!token.accepts(&new));

        std::fs::write(&path, &new).unwrap();
        // Another date, in case the file system rounds to the second.
        let later = SystemTime::now() + Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert!(token.accepts(&new));
        assert!(
            token.accepts(&old),
            "the previous one is valid for a few minutes"
        );

        // A short one (half written) does not replace the current one.
        std::fs::write(&path, "short").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later + Duration::from_secs(5))
            .unwrap();
        assert!(token.accepts(&new));
        assert!(InternalToken::from_file(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
