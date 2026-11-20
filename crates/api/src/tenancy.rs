//! The tenant of each request (online edition).
//!
//! Every company comes in through its subdomain, `<slug>.<domain>`. This
//! middleware looks it up in the directory and runs the request inside that
//! tenant ([`tenancy::within`]): from there the Postgres pool sets the tenant
//! on every connection and RLS filters the rows. On-premise everything belongs
//! to the main tenant.
//!
//! - A subdomain nobody owns (or a closed company's): `404`.
//! - A company in another cell: `421`, the router sent it to the wrong API.
//! - A suspended company: read-only (`403` on writes).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tenancy::{Directory, Mode, Status, Tenant, TenantRecord};

use crate::error::ApiError;

/// How long what the directory said about a subdomain is remembered.
const CACHE_TTL: Duration = Duration::from_secs(30);

type Cache = Mutex<HashMap<String, (Option<TenantRecord>, Instant)>>;

/// How the tenant of the requests is resolved.
#[derive(Clone)]
pub struct TenantResolver {
    mode: Mode,
    /// `orquestador.app`: companies come in through `<slug>.orquestador.app`.
    domain: String,
    /// The cell this API serves.
    cell: String,
    directory: Option<Arc<dyn Directory>>,
    cache: Arc<Cache>,
}

impl std::fmt::Debug for TenantResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TenantResolver")
            .field("mode", &self.mode)
            .field("domain", &self.domain)
            .field("cell", &self.cell)
            .finish_non_exhaustive()
    }
}

impl TenantResolver {
    /// On-premise: everything belongs to the main tenant.
    pub fn single() -> Self {
        Self {
            mode: Mode::Single,
            domain: String::new(),
            cell: String::new(),
            directory: None,
            cache: Arc::default(),
        }
    }

    /// Online: the tenant comes from the subdomain.
    pub fn multiple(domain: &str, cell: &str, directory: Arc<dyn Directory>) -> Self {
        Self {
            mode: Mode::Multiple,
            domain: domain.trim().trim_matches('.').to_ascii_lowercase(),
            cell: cell.to_string(),
            directory: Some(directory),
            cache: Arc::default(),
        }
    }

    /// The subdomain of a `Host` (without port), if it is one of ours.
    fn slug<'a>(&self, host: &'a str) -> Option<&'a str> {
        let host = host.split(':').next().unwrap_or(host);
        let slug = host.strip_suffix(&self.domain)?.strip_suffix('.')?;
        (!slug.is_empty() && !slug.contains('.')).then_some(slug)
    }

    async fn find(&self, slug: &str) -> Result<Option<TenantRecord>, String> {
        if let Ok(cache) = self.cache.lock() {
            if let Some((record, at)) = cache.get(slug) {
                if at.elapsed() < CACHE_TTL {
                    return Ok(record.clone());
                }
            }
        }
        let directory = self.directory.as_ref().ok_or("no tenant directory")?;
        let record = directory.by_slug(slug).await?;
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(slug.to_string(), (record.clone(), Instant::now()));
        }
        Ok(record)
    }
}

fn not_found() -> Response {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "tenant_not_found",
        "No hay una empresa en esta dirección.",
    )
    .into_response()
}

/// What a suspended company can do: look and sign out.
fn read_only(request: &Request) -> bool {
    matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) || request.uri().path() == "/auth/logout"
}

/// The middleware: resolve the tenant and run the request inside it.
pub async fn resolve(
    State(tenants): State<TenantResolver>,
    request: Request,
    next: Next,
) -> Response {
    if tenants.mode == Mode::Single {
        return within(Tenant::main(), next.run(request)).await;
    }
    // Probes belong to no company.
    if matches!(request.uri().path(), "/health" | "/ready") {
        return next.run(request).await;
    }
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .or_else(|| request.uri().host())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let Some(slug) = tenants.slug(&host) else {
        return not_found();
    };
    // The common entrance: outside every company, and only to sign in.
    if slug == tenancy::ENTRY {
        return if matches!(request.uri().path(), "/auth/central" | "/auth/captcha") {
            next.run(request).await
        } else {
            not_found()
        };
    }
    let record = match tenants.find(slug).await {
        Ok(Some(r)) if r.status != Status::Closed => r,
        Ok(_) => return not_found(),
        Err(e) => {
            return ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "tenant_lookup_failed",
                "No se pudo identificar la empresa: intenta de nuevo.",
            )
            .with_internal(e)
            .into_response()
        }
    };
    if record.cell != tenants.cell {
        return ApiError::new(
            StatusCode::MISDIRECTED_REQUEST,
            "wrong_cell",
            "Esta empresa se atiende en otra celda.",
        )
        .into_response();
    }
    if record.status == Status::Suspended && !read_only(&request) {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "tenant_suspended",
            "La cuenta de esta empresa está suspendida: solo se puede consultar.",
        )
        .into_response();
    }
    within(record.tenant, next.run(request)).await
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
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::routing::{get, post};
    use axum::Router;
    use tower::ServiceExt;

    struct Fixed;

    #[async_trait]
    impl Directory for Fixed {
        async fn by_slug(&self, slug: &str) -> Result<Option<TenantRecord>, String> {
            let (status, cell) = match slug {
                "acme" => (Status::Active, "main"),
                "late-payer" => (Status::Suspended, "main"),
                "old" => (Status::Closed, "main"),
                "big" => (Status::Active, "dedicated-1"),
                _ => return Ok(None),
            };
            Ok(Some(TenantRecord {
                tenant: Tenant {
                    id: format!("id-{slug}"),
                    slug: slug.into(),
                },
                name: slug.into(),
                status,
                cell: cell.into(),
            }))
        }
        async fn in_cell(&self, _: &str) -> Result<Vec<TenantRecord>, String> {
            Ok(Vec::new())
        }
    }

    fn app(tenants: TenantResolver) -> Router {
        let who = || async { tenancy::current().map(|t| t.slug).unwrap_or_default() };
        Router::new()
            .route("/thing", get(who).post(who))
            .route("/health", get(who))
            .route("/auth/logout", post(who))
            .route("/auth/central", post(who))
            .layer(axum::middleware::from_fn_with_state(tenants, resolve))
    }

    async fn call(tenants: TenantResolver, method: &str, host: &str, path: &str) -> (u16, String) {
        let response = app(tenants)
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::HOST, host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status().as_u16();
        let body = axum::body::to_bytes(response.into_body(), 1 << 16)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    fn online() -> TenantResolver {
        TenantResolver::multiple("Orquestador.app", "main", Arc::new(Fixed))
    }

    #[tokio::test]
    async fn the_subdomain_picks_the_tenant() {
        assert_eq!(
            call(online(), "GET", "acme.orquestador.app", "/thing").await,
            (200, "acme".into())
        );
        assert_eq!(
            call(online(), "GET", "ACME.orquestador.app:443", "/thing").await,
            (200, "acme".into())
        );
    }

    #[tokio::test]
    async fn unknown_foreign_or_closed_hosts_are_not_found() {
        for host in [
            "nobody.orquestador.app",
            "old.orquestador.app",
            "orquestador.app",
            "a.b.orquestador.app",
            "acme.other.com",
            "acmeorquestador.app",
        ] {
            let (status, _) = call(online(), "GET", host, "/thing").await;
            assert_eq!(status, 404, "{host}");
        }
    }

    #[tokio::test]
    async fn the_common_entrance_has_no_tenant_and_only_signs_in() {
        let host = "entrar.orquestador.app";
        assert_eq!(
            call(online(), "POST", host, "/auth/central").await,
            (200, String::new())
        );
        assert_eq!(call(online(), "GET", host, "/thing").await.0, 404);
        assert_eq!(call(online(), "POST", host, "/auth/logout").await.0, 404);
    }

    #[tokio::test]
    async fn another_cell_is_misdirected() {
        let (status, _) = call(online(), "GET", "big.orquestador.app", "/thing").await;
        assert_eq!(status, 421);
    }

    #[tokio::test]
    async fn a_suspended_tenant_can_only_read_and_sign_out() {
        let host = "late-payer.orquestador.app";
        assert_eq!(call(online(), "GET", host, "/thing").await.0, 200);
        assert_eq!(call(online(), "POST", host, "/thing").await.0, 403);
        assert_eq!(call(online(), "POST", host, "/auth/logout").await.0, 200);
    }

    #[tokio::test]
    async fn probes_need_no_tenant_and_on_premise_is_always_the_main_tenant() {
        assert_eq!(
            call(online(), "GET", "10.0.0.5:8080", "/health").await,
            (200, String::new())
        );
        assert_eq!(
            call(TenantResolver::single(), "GET", "anything", "/thing").await,
            (200, "main".into())
        );
    }

    /// Every line of a request carries its company's id (decision 0026),
    /// and one request id though `api::router`'s own context is inside the
    /// server's. Synthetic values only.
    #[test]
    fn a_requests_lines_carry_its_tenants_id_and_one_request_id() {
        let logged = || async {
            tracing::info!("in the handler");
        };
        let context = || axum::middleware::from_fn(crate::telemetry::request_context);
        let app = Router::new()
            .route("/thing", get(logged))
            .layer(context())
            .layer(axum::middleware::from_fn_with_state(online(), resolve))
            .layer(context());
        let request = Request::get("/thing")
            .header(header::HOST, "acme.orquestador.app")
            .body(Body::empty())
            .unwrap();
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
            assert_eq!(line["tenant"], "id-acme", "{line:?}");
            assert_eq!(line["request_id"], echoed.to_str().unwrap());
        }
        assert_eq!(lines[1]["msg"], "request completed");
        assert!(lines[1]["duration_ms"].is_f64(), "{lines:?}");
    }
}
