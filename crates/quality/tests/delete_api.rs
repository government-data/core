//! Deleting a dataset or a table of the lake (decisions 0055 and 0061), on
//! an ephemeral PostgreSQL, the catalog and the lake in memory: a dataset
//! goes whole (its bronze and silver tables, its rows, its mapping on
//! `main`) and another stays; a loose lake table goes; the name typed
//! again, a running cut, a table a dataset reads and the permission are
//! checked first. Synthetic names and rows only.

pub mod worker;

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Extension;
use axum::Router;
use contracts::{Access, AuditEntry, AuditTrail, Caller, Change, Module, ModuleRegistry};
use infrastructure::module_migrations::PgModuleMigrator;
use quality::catalog::{Catalog, CatalogSnapshot, DatasetMapping};
use quality::draft_api::{Approval, DraftCatalog, DraftFiles, DraftInfo, FileEdit, FileSaved};
use quality::module::QualityModule;
use quality::source_store::Sealer;
use quality::worker_api::WorkerServices;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

const MAIN: &str = "00000000-0000-0000-0000-000000000001";

fn mapping(dataset: &str) -> String {
    format!("dataset: {dataset}\nsource: {dataset}\nkey: [Id]\nfields:\n  Id: {{ column: id, type: text }}\n")
}

/// `main`: the datasets `loans` and `cards`, and those removed from it.
#[derive(Default)]
struct Main(Mutex<Vec<String>>);
#[async_trait::async_trait]
impl Catalog for Main {
    async fn snapshot(&self, dataset: &str) -> Result<Option<CatalogSnapshot>, String> {
        let there = ["loans", "cards"].contains(&dataset)
            && !self.0.lock().unwrap().iter().any(|d| d == dataset);
        Ok(there.then(|| CatalogSnapshot {
            commit: "c1".into(),
            mapping: mapping(dataset),
            rules: Vec::new(),
            others: Vec::new(),
        }))
    }
    async fn mappings(&self) -> Result<Vec<DatasetMapping>, String> {
        Ok(["loans", "cards"]
            .iter()
            .map(|d| DatasetMapping {
                dataset: d.to_string(),
                yaml: mapping(d),
            })
            .collect())
    }
}
#[async_trait::async_trait]
impl DraftCatalog for Main {
    async fn list(&self, _: &str) -> Result<Vec<DraftInfo>, String> {
        Ok(Vec::new())
    }
    async fn read(&self, _: &str, _: &str) -> Result<Option<DraftFiles>, String> {
        Ok(None)
    }
    async fn approve(&self, _: &str, _: &str, _: &str) -> Result<Approval, String> {
        unreachable!("not here")
    }
    async fn discard(&self, _: &str) -> Result<bool, String> {
        unreachable!("not here")
    }
    async fn save_file(&self, _: &FileEdit<'_>, _: bool) -> Result<FileSaved, String> {
        unreachable!("not here")
    }
    async fn remove_dataset(&self, dataset: &str, _: &str) -> Result<bool, String> {
        self.0.lock().unwrap().push(dataset.into());
        Ok(true)
    }
}

/// Manages `loans`, and every dataset if `all`.
struct Manager {
    all: bool,
}
impl Access for Manager {
    fn can(&self, action: &str, resource: &str) -> bool {
        action == "quality:ManageDatasets" && (resource == "dataset/loans" || self.all)
    }
    fn can_some(&self, action: &str) -> bool {
        self.can(action, "dataset/loans")
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

struct Plain;
impl Sealer for Plain {
    fn seal(&self, plain: &str) -> Result<String, String> {
        Ok(plain.into())
    }
    fn open(&self, sealed: &str) -> Result<String, String> {
        Ok(sealed.into())
    }
}

struct Fixture {
    db: testkit::TestDb,
    router: Router,
    main: Arc<Main>,
    tables: Arc<worker::Tables>,
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
        "ALTER TABLE quality.jobs OWNER TO app",
        "ALTER TABLE quality.schedules OWNER TO app",
        "ALTER TABLE quality.cuts OWNER TO app",
        "ALTER TABLE quality.run_summary OWNER TO app",
        "ALTER TABLE quality.rule_results OWNER TO app",
        "ALTER TABLE quality.lake_tables OWNER TO app",
    ] {
        sqlx::query(sql).execute(&db.pool).await.unwrap();
    }
    // Rows of `loans` and of `cards`, as cuts and schedules leave them.
    for dataset in ["loans", "cards"] {
        for sql in [
            "INSERT INTO quality.schedules (tenant_id, cron, fires, dataset) VALUES ($1::uuid, '0 6 * * *', 'cut', $2)",
            "INSERT INTO quality.cuts (tenant_id, dataset, cut, state, due_at) VALUES ($1::uuid, $2, DATE '2026-11-01', 'done', now())",
            "INSERT INTO quality.run_summary (tenant_id, dataset, cut, job_id, records, duration_ms, rules_commit)
             VALUES ($1::uuid, $2, DATE '2026-11-01', gen_random_uuid(), 3, 1, 'c1')",
            "INSERT INTO quality.rule_results (tenant_id, dataset, cut, rule, applies, failures)
             VALUES ($1::uuid, $2, DATE '2026-11-01', 'R1', 3, 1)",
        ] {
            sqlx::query(sql).bind(MAIN).bind(dataset).execute(&db.pool).await.unwrap();
        }
    }
    let app = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    let main = Arc::new(Main::default());
    let tables = Arc::new(worker::Tables::default());
    for t in [
        "loans_bronze.loans",
        "loans_silver.loans",
        "cards_bronze.cards",
        "cards_silver.cards",
        "loose_bronze.orphan",
    ] {
        tables.0.lock().unwrap().push((t.into(), "tables/x".into()));
    }
    let services = WorkerServices {
        tokens: Arc::new(worker::Tokens),
        lake: None,
        catalog: None,
        manifests: None,
        sources: None,
        issues: None,
        tables: Some(tables.clone()),
        drafts: None,
        proposal_alerts: None,
    };
    let router = QualityModule::serving(app.clone(), services)
        .with_sources(app, Arc::new(Plain), Some(main.clone() as Arc<dyn Catalog>))
        .with_drafts(main.clone())
        .user_router();
    Fixture {
        db,
        router,
        main,
        tables,
        log: Arc::default(),
    }
}

impl Fixture {
    async fn delete(&self, all: bool, uri: &str, confirm: &str) -> (StatusCode, Value) {
        let caller = Caller::new(
            "00000000-0000-0000-0000-0000000000a1".into(),
            None,
            "192.0.2.1".into(),
            None,
            Arc::new(Manager { all }),
            self.log.clone(),
        );
        let req = Request::builder()
            .method("DELETE")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(json!({ "confirm": confirm }).to_string()))
            .unwrap();
        let res = self
            .router
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

    async fn rows(&self, dataset: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM quality.schedules WHERE dataset = $1)
                  + (SELECT count(*) FROM quality.cuts WHERE dataset = $1)
                  + (SELECT count(*) FROM quality.run_summary WHERE dataset = $1)
                  + (SELECT count(*) FROM quality.rule_results WHERE dataset = $1)",
        )
        .bind(dataset)
        .fetch_one(&self.db.pool)
        .await
        .unwrap()
    }

    fn in_lake(&self, table: &str) -> bool {
        self.tables
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|(t, _)| t == table)
    }
}

#[tokio::test]
async fn a_dataset_is_deleted_whole_and_another_stays() {
    let f = fixture().await;
    assert_eq!(f.rows("loans").await, 4);
    let (status, out) = f.delete(false, "/quality/datasets/loans", "loan").await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("confirm"))
    );
    assert_eq!(
        f.delete(false, "/quality/datasets/cards", "cards").await.0,
        StatusCode::FORBIDDEN
    );

    sqlx::query("UPDATE quality.cuts SET state = 'extracting' WHERE dataset = 'loans'")
        .execute(&f.db.pool)
        .await
        .unwrap();
    let (status, out) = f.delete(false, "/quality/datasets/loans", "loans").await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::CONFLICT, Some("running"))
    );
    sqlx::query("UPDATE quality.cuts SET state = 'done' WHERE dataset = 'loans'")
        .execute(&f.db.pool)
        .await
        .unwrap();

    let (status, _) = f.delete(false, "/quality/datasets/loans", "loans").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(f.rows("loans").await, 0, "its schedules, cuts and results");
    assert!(!f.in_lake("loans_bronze.loans"), "its bronze table");
    assert!(!f.in_lake("loans_silver.loans"), "its silver table");
    assert_eq!(
        *f.main.0.lock().unwrap(),
        ["loans"],
        "its mapping and rules off main"
    );
    assert_eq!(f.rows("cards").await, 4, "another dataset stays");
    assert!(f.in_lake("cards_bronze.cards") && f.in_lake("cards_silver.cards"));
    let last = f.log.0.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        (last.action, last.resource.as_str(), last.change),
        ("quality:ManageDatasets", "dataset/loans", Change::Deleted)
    );
    assert_eq!(
        f.delete(false, "/quality/datasets/loans", "loans").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_loose_lake_table_is_deleted_and_one_a_dataset_reads_is_not() {
    let f = fixture().await;
    let loose = "/quality/lake/tables/loose_bronze/orphan";
    assert_eq!(
        f.delete(false, loose, "loose_bronze.orphan").await.0,
        StatusCode::FORBIDDEN,
        "every dataset's manager"
    );
    let (status, out) = f.delete(true, loose, "orphan").await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("confirm"))
    );
    let (status, out) = f
        .delete(
            true,
            "/quality/lake/tables/cards_bronze/cards",
            "cards_bronze.cards",
        )
        .await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::CONFLICT, Some("in_use"))
    );
    assert!(f.in_lake("cards_bronze.cards"));
    let (status, _) = f
        .delete(
            true,
            "/quality/lake/tables/cards_silver/cards",
            "cards_silver.cards",
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "its silver table too");

    assert_eq!(
        f.delete(true, loose, "loose_bronze.orphan").await.0,
        StatusCode::NO_CONTENT
    );
    assert!(!f.in_lake("loose_bronze.orphan"));
    let last = f.log.0.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        (last.resource.as_str(), last.change),
        ("lake/loose_bronze.orphan", Change::Deleted)
    );
    assert_eq!(
        f.delete(true, loose, "loose_bronze.orphan").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_namespace_goes_with_all_its_tables_unless_a_dataset_reads_one() {
    let f = fixture().await;
    f.tables
        .0
        .lock()
        .unwrap()
        .push(("loose_bronze.other".into(), "tables/y".into()));
    let (status, out) = f
        .delete(
            true,
            "/quality/lake/namespaces/cards_bronze",
            "cards_bronze",
        )
        .await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::CONFLICT, Some("in_use"))
    );
    let loose = "/quality/lake/namespaces/loose_bronze";
    assert_eq!(
        f.delete(false, loose, "loose_bronze").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.delete(true, loose, "loose").await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        f.delete(true, loose, "loose_bronze").await.0,
        StatusCode::NO_CONTENT
    );
    assert!(
        !f.in_lake("loose_bronze.orphan") && !f.in_lake("loose_bronze.other"),
        "every table of it"
    );
    assert!(f.in_lake("cards_bronze.cards"), "another namespace stays");
    let last = f.log.0.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        (last.resource.as_str(), last.change),
        ("lake/loose_bronze", Change::Deleted)
    );
    assert_eq!(
        f.delete(true, loose, "loose_bronze").await.0,
        StatusCode::NOT_FOUND
    );
}
