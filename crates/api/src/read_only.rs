//! Read-only by license: with the contract expired (past the grace period) or
//! suspended, everything can be looked at and what is scheduled keeps running,
//! but nothing is created, changed or deployed. A cooperative's processes are
//! never cut off over a late payment.

use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use application::PlatformService;

use crate::error::ApiError;

/// What can be done even when the installation is read-only: signing in and
/// out, the license (to renew it), cancelling an execution, testing the
/// settings, and a machine renewing its certificate: without it, a machine
/// stops within a day and what is scheduled with it (decision 0066),
/// Trino's coordinator holding its jobs to restart with its new one, and
/// Grafana's alerts about the machines (0072).
fn allowed(method: &Method, path: &str) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return true;
    }
    path.starts_with("/auth/")
        || path.starts_with("/license/")
        || (path.starts_with("/executions/") && path.ends_with("/cancel"))
        || (path.starts_with("/settings/") && path.ends_with("/test"))
        || path == crate::machines::renew::RENEW_PATH
        || path == crate::machines::trino::HOLD_PATH
        || path == crate::machines::trino::RELEASE_PATH
        || path == crate::machines::dolt::HOLD_PATH
        || path == crate::machines::dolt::RELEASE_PATH
        || path == crate::machines::alerts::ALERTS_PATH
}

pub async fn by_license(
    State(platform): State<PlatformService>,
    request: Request,
    next: Next,
) -> Response {
    if !allowed(request.method(), request.uri().path()) {
        if let Some(reason) = platform.read_only() {
            return ApiError::new(
                StatusCode::FORBIDDEN,
                "license_read_only",
                texts::tr!("api.license.readOnly", reason = reason),
            )
            .into_response();
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_running_and_renewing_stay_allowed() {
        let yes = |m: Method, p: &str| assert!(allowed(&m, p), "{m} {p}");
        let no = |m: Method, p: &str| assert!(!allowed(&m, p), "{m} {p}");
        yes(Method::GET, "/workflows");
        yes(Method::POST, "/auth/logout");
        yes(Method::PUT, "/license/key");
        yes(Method::POST, "/executions/1/cancel");
        yes(Method::POST, "/settings/mail/test");
        yes(Method::POST, "/internal/machines/renew");
        no(Method::POST, "/internal/machines/renew/x");
        yes(Method::POST, "/internal/machines/trino/hold");
        yes(Method::POST, "/internal/machines/trino/release");
        no(Method::POST, "/internal/machines/trino/other");
        yes(Method::POST, "/internal/machines/dolt/hold");
        yes(Method::POST, "/internal/machines/dolt/release");
        yes(Method::POST, "/internal/machines/alerts");
        yes(Method::GET, crate::machines::lake::CHECK_PATH);
        no(Method::POST, "/install/join");
        no(Method::POST, "/workflows/1/run");
        no(Method::POST, "/uploads");
        no(Method::PUT, "/settings/ldap");
        no(Method::DELETE, "/secrets/1");
    }
}
