//! The quality routes that read the catalog, with none and with one that
//! fails (decision 0026, amended 2026-11-24): `catalog_not_configured`
//! when the installation has no catalog, the drafts' routes included;
//! `catalog_unavailable` when it is there and fails. On an ephemeral
//! PostgreSQL, driven without a socket. Synthetic values only.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use contracts::{Access, AuditEntry, AuditTrail, Caller, Module, ModuleRegistry};
use infrastructure::module_migrations::PgModuleMigrator;
use quality::catalog::{Catalog, CatalogSnapshot, DatasetMapping};
use quality::module::QualityModule;
use quality::source_store::Sealer;
use serde_json::{json, Value};
use tower::ServiceExt;

/// Seals nothing: no source is written here.
struct Plain;
impl Sealer for Plain {
    fn seal(&self, plain: &str) -> Result<String, String> {
        Ok(plain.to_string())
    }
    fn open(&self, sealed: &str) -> Result<String, String> {
        Ok(sealed.to_string())
    }
}

/// May view anything.
struct Viewer;
impl Access for Viewer {
    fn can(&self, action: &str, _resource: &str) -> bool {
        action.starts_with("quality:View")
    }
    fn can_some(&self, action: &str) -> bool {
        self.can(action, "")
    }
}

struct NoAudit;
#[async_trait::async_trait]
impl AuditTrail for NoAudit {
    async fn record(&self, _: &Caller, _: &AuditEntry) -> Result<(), String> {
        Ok(())
    }
}

/// A catalog that is there and fails.
struct Failing;
#[async_trait::async_trait]
impl Catalog for Failing {
    async fn snapshot(&self, _dataset: &str) -> Result<Option<CatalogSnapshot>, String> {
        Err("connection refused".into())
    }
    async fn mappings(&self) -> Result<Vec<DatasetMapping>, String> {
        Err("connection refused".into())
    }
}

async fn router(db: &testkit::TestDb, catalog: Option<Arc<dyn Catalog>>) -> Router {
    // The queue's migration grants to it.
    sqlx::query("CREATE ROLE qd_queue LOGIN PASSWORD 'queue'")
        .execute(&db.pool)
        .await
        .unwrap();
    let mut migrator = PgModuleMigrator::load(&db.pool).await.unwrap();
    ModuleRegistry::new()
        .register(QualityModule::default())
        .run_migrations(&mut migrator)
        .unwrap();
    migrator.run(&db.pool).await.unwrap();
    QualityModule::default()
        .with_sources(db.pool.clone(), Arc::new(Plain), catalog)
        .user_router()
}

async fn get(router: &Router, uri: &str) -> (StatusCode, Value) {
    let caller = Caller::new(
        "00000000-0000-0000-0000-0000000000a1".into(),
        None,
        "192.0.2.1".into(),
        None,
        Arc::new(Viewer),
        Arc::new(NoAudit),
    );
    let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
    let res = router
        .clone()
        .layer(Extension(caller))
        .oneshot(req)
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn without_a_catalog_its_routes_say_there_is_none() {
    let db = testkit::spawn_test_db().await;
    let router = router(&db, None).await;
    let none = (
        StatusCode::SERVICE_UNAVAILABLE,
        json!({ "error": "catalog_not_configured" }),
    );
    // The drafts' routes are served too, over no catalog.
    for uri in ["/quality/datasets", "/quality/drafts"] {
        assert_eq!(get(&router, uri).await, none, "{uri}");
    }
}

#[tokio::test]
async fn a_catalog_that_fails_is_unavailable() {
    let db = testkit::spawn_test_db().await;
    let router = router(&db, Some(Arc::new(Failing))).await;
    let unavailable = (
        StatusCode::SERVICE_UNAVAILABLE,
        json!({ "error": "catalog_unavailable" }),
    );
    assert_eq!(get(&router, "/quality/datasets").await, unavailable);
}
