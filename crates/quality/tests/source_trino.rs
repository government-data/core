//! A source on Trino keeps its catalog (decision 0048, point 1), on an
//! ephemeral PostgreSQL with a Trino that records its statements: saved, the
//! catalog is made again with the current credentials; deleted, it is
//! dropped first; Trino refusing is said, and a source on DuckDB never
//! reaches Trino. Synthetic values only.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use contracts::{Access, AuditEntry, AuditTrail, Caller, Change, Module, ModuleRegistry};
use infrastructure::module_migrations::PgModuleMigrator;
use quality::catalog::{Catalog, CatalogSnapshot, DatasetMapping};
use quality::module::QualityModule;
use quality::source_catalog::Trino;
use quality::source_store::Sealer;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use uuid::Uuid;

struct Plain;
impl Sealer for Plain {
    fn seal(&self, plain: &str) -> Result<String, String> {
        Ok(plain.to_string())
    }
    fn open(&self, sealed: &str) -> Result<String, String> {
        Ok(sealed.to_string())
    }
}

struct Everything;
impl Access for Everything {
    fn can(&self, _: &str, _: &str) -> bool {
        true
    }
    fn can_some(&self, _: &str) -> bool {
        true
    }
}

#[derive(Default)]
struct Log(Mutex<Vec<AuditEntry>>);
#[async_trait::async_trait]
impl AuditTrail for Log {
    async fn record(&self, _: &Caller, entry: &AuditEntry) -> Result<(), String> {
        self.0.lock().unwrap().push(entry.clone());
        Ok(())
    }
}

/// No mapping reads any source.
struct NoMappings;
#[async_trait::async_trait]
impl Catalog for NoMappings {
    async fn snapshot(&self, _: Uuid, _: &str) -> Result<Option<CatalogSnapshot>, String> {
        Ok(None)
    }
    async fn mappings(&self, _: Uuid) -> Result<Vec<DatasetMapping>, String> {
        Ok(Vec::new())
    }
}

/// Records each statement; refuses them all while `down`.
#[derive(Default)]
struct Recorder {
    statements: Mutex<Vec<String>>,
    down: Mutex<bool>,
}
#[async_trait::async_trait]
impl Trino for Recorder {
    async fn run(&self, sql: &str) -> Result<(), String> {
        if *self.down.lock().unwrap() {
            return Err("unavailable".into());
        }
        self.statements.lock().unwrap().push(sql.to_string());
        Ok(())
    }
}

impl Recorder {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.statements.lock().unwrap())
    }
}

struct Fixture {
    _db: testkit::TestDb,
    router: Router,
    log: Arc<Log>,
    trino: Arc<Recorder>,
}

async fn fixture() -> Fixture {
    let db = testkit::spawn_test_db().await;
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
    let options = testkit::app_role(&db).await;
    for sql in [
        "GRANT USAGE ON SCHEMA quality TO app",
        "ALTER TABLE quality.sources OWNER TO app",
        "ALTER TABLE quality.source_tables OWNER TO app",
        "ALTER TABLE quality.jobs OWNER TO app",
    ] {
        sqlx::query(sql).execute(&db.pool).await.unwrap();
    }
    let app = infrastructure::db::with_tenancy(PgPoolOptions::new(), tenancy::Mode::Single)
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    let trino = Arc::new(Recorder::default());
    let router = QualityModule::default()
        .with_sources(app, Arc::new(Plain), Some(Arc::new(NoMappings)))
        .with_trino(trino.clone())
        .user_router();
    Fixture {
        _db: db,
        router,
        log: Arc::new(Log::default()),
        trino,
    }
}

impl Fixture {
    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let caller = Caller::new(
            "00000000-0000-0000-0000-0000000000a1".into(),
            None,
            "192.0.2.1".into(),
            None,
            Arc::new(Everything),
            self.log.clone(),
        );
        let mut req = Request::builder().method(method).uri(uri);
        if body.is_some() {
            req = req.header("content-type", "application/json");
        }
        let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
        let res = self
            .router
            .clone()
            .layer(Extension(caller))
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, body)
    }

    fn changes(&self) -> Vec<Change> {
        self.log
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.change)
            .collect()
    }
}

fn ledger(host: &str, password: Option<&str>) -> Value {
    let mut secrets = json!({ "user": "reader" });
    if let Some(p) = password {
        secrets["password"] = json!(p);
    }
    json!({
        "driver": "sqlserver",
        "fields": { "host": host, "port": "1433", "database": "ledger" },
        "secrets": secrets,
    })
}

const DROP: &str = "DROP CATALOG IF EXISTS src_ledger";

#[tokio::test]
async fn a_source_on_trino_keeps_its_catalog_with_its_current_credentials() {
    let f = fixture().await;
    let uri = "/quality/sources/ledger";
    let created = f.call("PUT", uri, Some(ledger("db.example", Some("synthetic"))));
    assert_eq!(created.await.0, StatusCode::CREATED);
    let made = f.trino.take();
    assert_eq!(made[0], DROP);
    assert!(made[1].starts_with("CREATE CATALOG src_ledger USING sqlserver"));
    assert!(made[1].contains("'jdbc:sqlserver://db.example:1433;databaseName=ledger'"));
    assert!(made[1].contains("'synthetic'"));

    // Edited with its password left out: the catalog keeps the saved one.
    let edited = f.call("PUT", uri, Some(ledger("db2.example", None))).await;
    assert_eq!(edited.0, StatusCode::NO_CONTENT);
    let made = f.trino.take();
    assert_eq!(made.len(), 2);
    assert!(made[1].contains("db2.example") && made[1].contains("'synthetic'"));

    assert_eq!(f.call("DELETE", uri, None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(f.trino.take(), [DROP]);
    assert_eq!(
        f.changes(),
        [Change::Created, Change::Changed, Change::Deleted]
    );
}

#[tokio::test]
async fn trino_refusing_is_said_and_no_catalog_outlives_its_source() {
    let f = fixture().await;
    let uri = "/quality/sources/ledger";
    *f.trino.down.lock().unwrap() = true;
    // Saved and audited; saving it again retries its catalog.
    let (status, body) = f
        .call("PUT", uri, Some(ledger("db.example", Some("synthetic"))))
        .await;
    assert_eq!(
        (status, body),
        (StatusCode::BAD_GATEWAY, json!({ "error": "trino" }))
    );
    assert_eq!(f.changes(), [Change::Created]);

    // Its catalog cannot be dropped: the source is not deleted either.
    assert_eq!(f.call("DELETE", uri, None).await.0, StatusCode::BAD_GATEWAY);
    let (_, listed) = f.call("GET", "/quality/sources", None).await;
    assert_eq!(listed[0]["name"], "ledger");
    assert_eq!(f.changes(), [Change::Created]);
}

#[tokio::test]
async fn a_field_the_url_would_read_as_syntax_or_a_duckdb_source_never_reaches_trino() {
    let f = fixture().await;
    let (status, body) = f
        .call(
            "PUT",
            "/quality/sources/ledger",
            Some(ledger("db.example;user=x", Some("synthetic"))),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body,
        json!({ "errors": [{ "code": "value", "field": "host" }] })
    );
    let (_, listed) = f.call("GET", "/quality/sources", None).await;
    assert_eq!(listed, json!([]));

    let core = json!({
        "driver": "postgres",
        "fields": { "host": "db.example", "database": "core" },
        "secrets": { "user": "reader", "password": "synthetic" },
    });
    let uri = "/quality/sources/core";
    assert_eq!(f.call("PUT", uri, Some(core)).await.0, StatusCode::CREATED);
    assert_eq!(f.call("DELETE", uri, None).await.0, StatusCode::NO_CONTENT);
    assert!(f.trino.take().is_empty());
}
