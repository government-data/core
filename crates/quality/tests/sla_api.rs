//! A dataset's remediation SLAs (decision 0059) on an ephemeral
//! PostgreSQL, driven without a socket with the `Caller` server would put
//! in each request: read behind `quality:ViewResults`, replaced behind
//! `quality:EditPipelines` and audited, only on the mapping's dimensions,
//! only the tenant's. Synthetic names only.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use contracts::{Access, AuditEntry, AuditTrail, Caller, Change, Module, ModuleRegistry};
use infrastructure::module_migrations::PgModuleMigrator;
use quality::catalog::{Catalog, CatalogSnapshot, DatasetMapping};
use quality::module::QualityModule;
use quality::source_store::Sealer;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use uuid::Uuid;

const OTHER: &str = "00000000-0000-0000-0000-0000000000ff";
const URI: &str = "/quality/datasets/loans/queues/slas";

const LOANS: &str = "dataset: loans
source: loans_extract
key: [Loan]
fields:
  Loan: { column: loan_id, type: text }
  Office: { column: office, type: text, roles: [dimension] }
  Advisor: { column: advisor, type: text, roles: [dimension] }
  Amount: { column: amount, type: number }
";

struct NoSealer;
impl Sealer for NoSealer {
    fn seal(&self, _: &str) -> Result<String, String> {
        Err("unused".into())
    }
    fn open(&self, _: &str) -> Result<String, String> {
        Err("unused".into())
    }
}

/// What a caller may do on `pipeline/loans`; an editor edits `ghost`
/// too, a pipeline the catalog has no mapping for.
#[derive(Clone, Copy)]
enum Role {
    Nobody,
    Viewer,
    Editor,
    /// Edits pipelines but sees no results: editing still answers them.
    EditorOnly,
}
impl Access for Role {
    fn can(&self, action: &str, resource: &str) -> bool {
        if resource == "pipeline/ghost" {
            return matches!(self, Self::Editor) && action == "quality:EditPipelines";
        }
        resource == "pipeline/loans"
            && matches!(
                (self, action),
                (Self::Viewer | Self::Editor, "quality:ViewResults")
                    | (Self::Editor | Self::EditorOnly, "quality:EditPipelines")
            )
    }
    fn can_some(&self, action: &str) -> bool {
        self.can(action, "pipeline/loans")
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

/// Has `loans` only.
struct OneDataset;
#[async_trait::async_trait]
impl Catalog for OneDataset {
    async fn snapshot(&self, _: Uuid, dataset: &str) -> Result<Option<CatalogSnapshot>, String> {
        Ok((dataset == "loans").then(|| CatalogSnapshot {
            commit: "c1".into(),
            mapping: LOANS.into(),
            others: Vec::new(),
            rules: Vec::new(),
        }))
    }
    async fn mappings(&self, _: Uuid) -> Result<Vec<DatasetMapping>, String> {
        Ok(Vec::new())
    }
}

struct Fixture {
    db: testkit::TestDb,
    router: Router,
    log: Arc<Log>,
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
        "ALTER TABLE quality.remediation_slas OWNER TO app",
    ] {
        sqlx::query(sql).execute(&db.pool).await.unwrap();
    }
    // Another tenant's SLA for a dataset of the same name.
    sqlx::query(
        "INSERT INTO quality.remediation_slas (tenant_id, dataset, dimension, grp, days)
         VALUES ($1::uuid, 'loans', 'Office', NULL, 3)",
    )
    .bind(OTHER)
    .execute(&db.pool)
    .await
    .unwrap();
    let app = infrastructure::db::with_tenancy(PgPoolOptions::new(), tenancy::Mode::Single)
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap();
    let router = QualityModule::default()
        .with_sources(app, Arc::new(NoSealer), Some(Arc::new(OneDataset)))
        .user_router();
    Fixture {
        db,
        router,
        log: Arc::new(Log::default()),
    }
}

impl Fixture {
    async fn call(
        &self,
        role: Role,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let caller = Caller::new(
            "00000000-0000-0000-0000-0000000000a1".into(),
            None,
            "192.0.2.1".into(),
            None,
            Arc::new(role),
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
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn audited(&self) -> Vec<(&'static str, String, Change)> {
        self.log
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|e| (e.action, e.resource.clone(), e.change))
            .collect()
    }

    /// Every tenant's SLAs, as the owner sees them.
    async fn stored(&self) -> Vec<(String, String, Option<String>, i32)> {
        sqlx::query_as(
            "SELECT tenant_id::text, dimension, grp, days FROM quality.remediation_slas
              ORDER BY tenant_id, dimension, grp NULLS FIRST",
        )
        .fetch_all(&self.db.pool)
        .await
        .unwrap()
    }
}

fn office(days: i64) -> Value {
    json!({ "slas": [
        { "dimension": "Office", "days": days },
        { "dimension": "Office", "group": "North", "days": 7 },
        { "dimension": "Office", "group": "", "days": 60 },
    ] })
}

#[tokio::test]
async fn whoever_may_not_is_refused_reading_or_setting() {
    let f = fixture().await;
    let (status, _) = f.call(Role::Nobody, "GET", URI, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = f.call(Role::Viewer, "PUT", URI, Some(office(10))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "viewing is not editing");
    let (status, _) = f
        .call(
            Role::Editor,
            "GET",
            "/quality/datasets/cards/queues/slas",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "another pipeline");
    assert!(f.audited().is_empty());
    assert_eq!(f.stored().await.len(), 1, "only the other tenant's");
}

#[tokio::test]
async fn a_dataset_without_slas_reads_the_default_and_its_dimensions() {
    let f = fixture().await;
    let (status, out) = f.call(Role::Viewer, "GET", URI, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        out,
        json!({ "default_days": 30, "dimensions": ["Advisor", "Office"], "slas": [] }),
        "never another tenant's"
    );
}

#[tokio::test]
async fn an_editor_sets_slas_audited_and_reads_them_back() {
    let f = fixture().await;
    let (status, out) = f.call(Role::EditorOnly, "PUT", URI, Some(office(15))).await;
    assert_eq!(status, StatusCode::OK);
    let expected = json!([
        { "dimension": "Office", "group": null, "days": 15 },
        { "dimension": "Office", "group": "", "days": 60 },
        { "dimension": "Office", "group": "North", "days": 7 },
    ]);
    assert_eq!(out["slas"], expected);
    let (_, read) = f.call(Role::Viewer, "GET", URI, None).await;
    assert_eq!(read["slas"], expected);
    assert_eq!(
        f.audited(),
        [(
            "quality:EditPipelines",
            "pipeline/loans/queues/slas".to_string(),
            Change::Changed
        )]
    );

    // A second PUT replaces the set: the groups left out go back to the
    // dimension's default; the other tenant's stays.
    let only = json!({ "slas": [{ "dimension": "Advisor", "days": 20 }] });
    let (status, out) = f.call(Role::Editor, "PUT", URI, Some(only)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        out["slas"],
        json!([{ "dimension": "Advisor", "group": null, "days": 20 }])
    );
    let main = "00000000-0000-0000-0000-000000000001".to_string();
    assert_eq!(
        f.stored().await,
        [
            (main, "Advisor".into(), None, 20),
            (OTHER.into(), "Office".into(), None, 3),
        ]
    );
    assert_eq!(f.audited().len(), 2);

    let (status, out) = f
        .call(Role::Editor, "PUT", URI, Some(json!({ "slas": [] })))
        .await;
    assert_eq!((status, out["slas"].clone()), (StatusCode::OK, json!([])));
}

#[tokio::test]
async fn slas_that_do_not_hold_together_are_refused_and_change_nothing() {
    let f = fixture().await;
    let (status, _) = f.call(Role::Editor, "PUT", URI, Some(office(15))).await;
    assert_eq!(status, StatusCode::OK);
    for (body, code) in [
        (
            json!({ "slas": [{ "dimension": "Amount", "days": 5 }] }),
            "dimension",
        ),
        (
            json!({ "slas": [{ "dimension": "Office", "days": 0 }] }),
            "days",
        ),
        (
            json!({ "slas": [{ "dimension": "Office", "days": 366 }] }),
            "days",
        ),
        (
            json!({ "slas": [
                { "dimension": "Office", "group": "North", "days": 5 },
                { "dimension": "Office", "group": "North", "days": 6 },
            ] }),
            "duplicate",
        ),
    ] {
        let (status, out) = f.call(Role::Editor, "PUT", URI, Some(body)).await;
        assert_eq!(
            (status, out["error"].as_str()),
            (StatusCode::UNPROCESSABLE_ENTITY, Some(code))
        );
    }
    let (status, out) = f
        .call(
            Role::Editor,
            "PUT",
            "/quality/datasets/ghost/queues/slas",
            Some(json!({ "slas": [] })),
        )
        .await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("not_found")),
        "no mapping on main"
    );
    assert_eq!(
        f.stored().await.len(),
        4,
        "the three set and the other tenant's"
    );
    assert_eq!(f.audited().len(), 1, "only the change that held");
}
