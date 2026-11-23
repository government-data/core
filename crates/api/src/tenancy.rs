//! The tenant of each request: always the main one (decision 0076, one
//! installation is one organisation).
//!
//! This middleware runs every request inside the main tenant
//! ([`tenancy::within`]): from there the Postgres pool sets it on every
//! connection and RLS filters the rows, until the schema's contraction.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use tenancy::Tenant;

/// How the tenant of the requests is resolved: the main one.
#[derive(Clone, Debug)]
pub struct TenantResolver(());

impl TenantResolver {
    /// Everything belongs to the main tenant.
    pub fn single() -> Self {
        Self(())
    }
}

/// The middleware: run the request inside the main tenant.
pub async fn resolve(State(_): State<TenantResolver>, request: Request, next: Next) -> Response {
    within(Tenant::main(), next.run(request)).await
}

/// `f` inside `tenant`, its id recorded as the request span's `tenant`
/// (decision 0026, point 1): the id, never the slug.
async fn within<F: std::future::Future>(tenant: Tenant, f: F) -> F::Output {
    tracing::Span::current().record("tenant", tenant.id.as_str());
    tenancy::within(tenant, f).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    #[tokio::test]
    async fn every_request_is_the_main_tenants() {
        let who = || async { tenancy::current().map(|t| t.slug).unwrap_or_default() };
        let app =
            Router::new()
                .route("/thing", get(who))
                .layer(axum::middleware::from_fn_with_state(
                    TenantResolver::single(),
                    resolve,
                ));
        let response = app
            .oneshot(Request::get("/thing").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body = axum::body::to_bytes(response.into_body(), 1 << 16)
            .await
            .unwrap();
        assert_eq!(body, "main");
    }

    /// Every line of a request carries the tenant's id (decision 0026), and
    /// one request id though `api::router`'s own context is inside the
    /// server's.
    #[test]
    fn a_requests_lines_carry_the_tenants_id_and_one_request_id() {
        let logged = || async {
            tracing::info!("in the handler");
        };
        let context = || axum::middleware::from_fn(crate::telemetry::request_context);
        let app = Router::new()
            .route("/thing", get(logged))
            .layer(context())
            .layer(axum::middleware::from_fn_with_state(
                TenantResolver::single(),
                resolve,
            ))
            .layer(context());
        let request = Request::get("/thing").body(Body::empty()).unwrap();
        let mut echoed = None;
        let lines = logging::json_lines("server", || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            let response = runtime.block_on(app.oneshot(request)).unwrap();
            echoed = response.headers().get("x-request-id").cloned();
        });
        assert_eq!(lines.len(), 2, "{lines:?}");
        let echoed = echoed.unwrap();
        for line in &lines {
            assert_eq!(line["tenant"], tenancy::MAIN_ID, "{line:?}");
            assert_eq!(line["request_id"], echoed.to_str().unwrap());
        }
        assert_eq!(lines[1]["msg"], "request completed");
        assert!(lines[1]["duration_ms"].is_f64(), "{lines:?}");
    }
}
