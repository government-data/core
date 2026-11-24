//! The module's refusals, `{"error": <code>}`, in one place. A server
//! error also writes its cause (decision 0026): one line with the code, a
//! fixed word chosen here (never a value, a body or a key), in the
//! request's span, so it carries the request line's `request_id`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// `{"error": error}` with `status`; a `5xx` logs `error` as its cause.
pub(crate) fn refuse(status: StatusCode, error: &'static str) -> Response {
    refuse_because(status, error, error)
}

/// As [`refuse`], with a cause more precise than the client's code, as
/// `catalog_not_configured` behind `catalog_unavailable`.
pub(crate) fn refuse_because(
    status: StatusCode,
    error: &'static str,
    cause: &'static str,
) -> Response {
    if status.is_server_error() {
        tracing::warn!(status = status.as_u16(), cause, "request refused");
    }
    (status, Json(json!({ "error": error }))).into_response()
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value};

    use super::*;

    fn lines(
        status: StatusCode,
        error: &'static str,
        cause: &'static str,
    ) -> Vec<Map<String, Value>> {
        logging::json_lines("server", || {
            let span = tracing::info_span!("request", request_id = "0123abcd");
            let _span = span.enter();
            refuse_because(status, error, cause);
        })
    }

    #[test]
    fn a_server_error_logs_its_cause_in_the_requests_span() {
        let lines = lines(
            StatusCode::SERVICE_UNAVAILABLE,
            "catalog_unavailable",
            "catalog_not_configured",
        );
        assert_eq!(lines.len(), 1, "{lines:?}");
        let line = &lines[0];
        assert_eq!(line["level"], "WARN");
        assert_eq!(line["cause"], "catalog_not_configured");
        assert_eq!(line["status"], 503);
        assert_eq!(line["request_id"], "0123abcd");
    }

    #[test]
    fn a_clients_error_logs_nothing() {
        assert!(lines(StatusCode::FORBIDDEN, "forbidden", "forbidden").is_empty());
    }

    #[tokio::test]
    async fn the_body_is_the_code_only() {
        let response = refuse_because(
            StatusCode::SERVICE_UNAVAILABLE,
            "catalog_unavailable",
            "catalog_not_configured",
        );
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], br#"{"error":"catalog_unavailable"}"#);
    }
}
