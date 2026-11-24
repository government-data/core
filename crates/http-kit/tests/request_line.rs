//! The request's line, as `logging` writes it: its level follows the
//! status, and a server error carries the cause its handler gave. Every
//! value here is synthetic.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use http_kit::error::ApiError;
use http_kit::telemetry::{request_context, with_cause};
use serde_json::{Map, Value};
use tower::ServiceExt;

fn app() -> Router {
    Router::new()
        .route("/ok", get(|| async { "ok" }))
        .route("/continue", get(|| async { StatusCode::CONTINUE }))
        .route(
            "/redirect",
            get(|| async { (StatusCode::FOUND, [("location", "/login")]).into_response() }),
        )
        .route("/missing", get(|| async { StatusCode::NOT_FOUND }))
        .route("/plain", get(|| async { StatusCode::SERVICE_UNAVAILABLE }))
        .route("/caused", get(caused))
        .route(
            "/api-error",
            get(|| async { ApiError::internal("synthetic detail") }),
        )
        .layer(axum::middleware::from_fn(request_context))
}

async fn caused() -> Response {
    with_cause(
        StatusCode::SERVICE_UNAVAILABLE.into_response(),
        "catalog_not_configured",
    )
}

/// The request's own line of one `GET path` (the one with `status`).
fn request_line(path: &str) -> Map<String, Value> {
    let request = Request::get(path).body(Body::empty()).unwrap();
    let lines = logging::json_lines("server", || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(app().oneshot(request)).unwrap();
    });
    let mut own: Vec<_> = lines
        .into_iter()
        .filter(|l| l.contains_key("status"))
        .collect();
    assert_eq!(own.len(), 1, "one request line: {own:?}");
    own.remove(0)
}

#[test]
fn an_answer_or_a_redirect_is_info() {
    for (path, status) in [("/ok", 200), ("/continue", 100), ("/redirect", 302)] {
        let line = request_line(path);
        assert_eq!(line["level"], "INFO", "{path}: {line:?}");
        assert_eq!(line["msg"], "request completed", "{path}");
        assert_eq!(line["status"], status, "{path}");
        assert!(line.contains_key("bytes"), "{path}: {line:?}");
    }
}

#[test]
fn a_clients_error_is_a_warning() {
    let line = request_line("/missing");
    assert_eq!(line["level"], "WARN");
    assert_eq!(line["msg"], "client error");
    assert_eq!(line["status"], 404);
}

#[test]
fn a_server_error_is_an_error_with_its_cause_when_given() {
    let line = request_line("/caused");
    assert_eq!(line["level"], "ERROR");
    assert_eq!(line["msg"], "server error");
    assert_eq!(line["status"], 503);
    assert_eq!(line["cause"], "catalog_not_configured");
    assert!(line["request_id"].as_str().is_some_and(|s| !s.is_empty()));

    let line = request_line("/plain");
    assert_eq!(line["level"], "ERROR");
    assert!(!line.contains_key("cause"), "{line:?}");
}

#[test]
fn an_api_errors_code_is_its_cause_and_its_detail_stays_on_its_own_line() {
    let line = request_line("/api-error");
    assert_eq!(line["level"], "ERROR");
    assert_eq!(line["status"], 500);
    assert_eq!(line["cause"], "internal_error");
    assert!(!Value::Object(line).to_string().contains("synthetic detail"));
}
