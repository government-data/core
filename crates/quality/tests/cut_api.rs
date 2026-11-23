//! _Run now_ on an ephemeral PostgreSQL, driven without a socket, with the
//! `Caller` server would put in each request (decision 0035): a person
//! opens today's cut of a dataset the catalog has, with its extraction,
//! audited as a run started by hand; a second run blocks or replaces it,
//! never under a run. Synthetic names only.

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

/// Never asked: _Run now_ seals nothing.
struct NoSealer;
impl Sealer for NoSealer {
    fn seal(&self, _: &str) -> Result<String, String> {
        Err("unused".into())
    }
    fn open(&self, _: &str) -> Result<String, String> {
        Err("unused".into())
    }
}

/// May run `loans` and `ghost` (which the catalog lacks) only, or nothing.
struct Analyst(bool);
impl Access for Analyst {
    fn can(&self, action: &str, resource: &str) -> bool {
        self.0
            && matches!(action, "quality:RunPipeline" | "quality:ViewPipelines")
            && matches!(
                resource,
                "pipeline/loans" | "pipeline/ghost" | "pipeline/uploaded"
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

/// Has `loans`, read from a source, and `uploaded`, loaded into the lake.
struct OneDataset;
#[async_trait::async_trait]
impl Catalog for OneDataset {
    async fn snapshot(&self, dataset: &str) -> Result<Option<CatalogSnapshot>, String> {
        let mapping = match dataset {
            "loans" => "dataset: loans\nsource: loans\nfrom: { source: core, table: public.loans }\nkey: [Id]\nfields:\n  Id: { column: loan_id, type: text }\n",
            "uploaded" => "dataset: uploaded\nsource: uploaded\nkey: [Id]\nfields:\n  Id: { column: id, type: text }\n",
            _ => return Ok(None),
        };
        Ok(Some(CatalogSnapshot {
            commit: "c1".into(),
            mapping: mapping.into(),
            others: Vec::new(),
            rules: Vec::new(),
        }))
    }
    async fn mappings(&self) -> Result<Vec<DatasetMapping>, String> {
        // `loans` read from a source's table; `cards` by a mapping that does not read.
        let loans = "dataset: loans\nsource: loans\nfrom: { source: core, table: public.loans }\nkey: [Id]\nfields:\n  Id: { column: loan_id, type: text }\n";
        Ok(vec![
            DatasetMapping {
                dataset: "loans".into(),
                yaml: loans.into(),
            },
            DatasetMapping {
                dataset: "cards".into(),
                yaml: String::new(),
            },
        ])
    }
}

struct Fixture {
    db: testkit::TestDb,
    router: Router,
    log: Arc<Log>,
}

async fn fixture() -> Fixture {
    fixture_with(Some(Arc::new(OneDataset))).await
}

async fn fixture_with(catalog: Option<Arc<dyn Catalog>>) -> Fixture {
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
        "ALTER TABLE quality.sources OWNER TO app",
    ] {
        sqlx::query(sql).execute(&db.pool).await.unwrap();
    }
    let app = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    let router = QualityModule::default()
        .with_sources(app, Arc::new(NoSealer), catalog)
        .user_router();
    Fixture {
        db,
        router,
        log: Arc::new(Log::default()),
    }
}

impl Fixture {
    async fn get(&self, uri: &str) -> (StatusCode, Value) {
        let caller = Caller::new(
            "00000000-0000-0000-0000-0000000000a1".into(),
            None,
            "192.0.2.1".into(),
            None,
            Arc::new(Analyst(true)),
            self.log.clone(),
        );
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
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

    async fn run(&self, may: bool, dataset: &str, body: Option<Value>) -> (StatusCode, Value) {
        let caller = Caller::new(
            "00000000-0000-0000-0000-0000000000a1".into(),
            None,
            "192.0.2.1".into(),
            None,
            Arc::new(Analyst(may)),
            self.log.clone(),
        );
        let mut req = Request::builder()
            .method("POST")
            .uri(format!("/quality/datasets/{dataset}/cuts"));
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

    /// Today's cut of `loans`: its state, run and who asked for it.
    async fn cut(&self) -> (String, i32, Option<Uuid>) {
        sqlx::query_as("SELECT state, run, requested_by FROM quality.cuts WHERE dataset = 'loans'")
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    async fn finish_cut(&self) {
        sqlx::query("UPDATE quality.cuts SET state = 'done'")
            .execute(&self.db.pool)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_person_runs_todays_cut_audited_and_a_second_run_blocks_or_replaces() {
    let f = fixture().await;
    // `loans`' mapping reads `core`, which DuckDB reads.
    sqlx::query(
        "INSERT INTO quality.sources (name, driver, secrets) VALUES ('core', 'postgres', '')",
    )
    .execute(&f.db.pool)
    .await
    .unwrap();
    let (status, out) = f.run(true, "loans", None).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(out["outcome"], "queued");
    assert_eq!(out["cut"].as_str().unwrap().len(), 10, "YYYY-MM-DD");
    let (state, run, by) = f.cut().await;
    assert_eq!((state.as_str(), run), ("extracting", 1));
    assert_eq!(
        by.map(|u| u.to_string()).as_deref(),
        Some("00000000-0000-0000-0000-0000000000a1")
    );
    let extract: (String, Value, Option<bool>) =
        sqlx::query_as("SELECT kind, parameters, uses_trino FROM quality.jobs WHERE id = $1")
            .bind(Uuid::parse_str(out["extract_job"].as_str().unwrap()).unwrap())
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert_eq!(extract.0, "extract");
    assert_eq!(extract.1["cut"], out["cut"]);
    // No longer written (decision 0077): left NULL until it is dropped.
    assert_eq!(extract.2, None);

    // Under a run, even asked to replace: blocked, not audited.
    for body in [None, Some(json!({ "rerun": "replace" }))] {
        let (status, out) = f.run(true, "loans", body).await;
        assert_eq!(
            (status, out["outcome"].as_str()),
            (StatusCode::CONFLICT, Some("blocked"))
        );
    }
    // Done: replacing runs it again.
    f.finish_cut().await;
    let (status, out) = f
        .run(true, "loans", Some(json!({ "rerun": "replace" })))
        .await;
    assert_eq!(
        (status, out["outcome"].as_str()),
        (StatusCode::CREATED, Some("replaced"))
    );
    assert_eq!(f.cut().await.1, 2);

    let log = f.log.0.lock().unwrap().clone();
    assert_eq!(log.len(), 2, "each run started, nothing for a blocked one");
    assert!(log
        .iter()
        .all(|e| e.change == Change::Ran && e.resource == "pipeline/loans"));
}

#[tokio::test]
async fn what_a_person_may_not_run_or_the_catalog_lacks_is_refused() {
    let f = fixture().await;
    assert_eq!(f.run(false, "loans", None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(f.run(true, "others", None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(f.run(true, "ghost", None).await.0, StatusCode::NOT_FOUND);
    // Loaded into the lake, read from no source: nothing to extract.
    let (status, out) = f.run(true, "uploaded", None).await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("no_source"))
    );
    let (status, _) = f
        .run(true, "loans", Some(json!({ "rerun": "sometimes" })))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(f.log.0.lock().unwrap().is_empty());
    let blind = fixture_with(None).await;
    assert_eq!(
        blind.run(true, "loans", None).await.0,
        StatusCode::SERVICE_UNAVAILABLE,
        "without the catalog, no dataset is known"
    );
}

#[tokio::test]
async fn a_datasets_cuts_are_listed_newest_first_with_where_they_came_from() {
    let f = fixture().await;
    let (_, out) = f.run(true, "loans", None).await;
    let (status, mut datasets) = f.get("/quality/datasets").await;
    assert_eq!(status, StatusCode::OK);
    // Its latest cut: the one just opened (decision 0049, _no cuts yet_ without).
    let last_cut = datasets[0]["last_cut"].take();
    assert_eq!(
        datasets[0].as_object_mut().unwrap().remove("last_cut"),
        Some(serde_json::Value::Null)
    );
    assert_eq!(
        datasets,
        serde_json::json!([{
            "name": "loans",
            "from": { "source": "core", "schema": "public", "table": "loans" },
            "bronze": "loans",
            "fields": [{ "field": "Id", "column": "loan_id" }]
        }]),
        "not cards': not this analyst's; where loans is read from"
    );
    assert_eq!(
        f.get("/quality/datasets?for=rules").await.0,
        StatusCode::FORBIDDEN,
        "seeing a pipeline is not seeing its rules"
    );
    assert_eq!(
        f.get("/quality/datasets?for=everything").await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (status, cuts) = f.get("/quality/datasets/loans/cuts").await;
    assert_eq!(status, StatusCode::OK);
    let cut = &cuts[0];
    assert_eq!(cut["id"], out["cut_id"]);
    assert_eq!(last_cut, cut["cut"]);
    assert_eq!(
        (cut["state"].as_str(), cut["run"].as_i64()),
        (Some("extracting"), Some(1))
    );
    assert_eq!(cut["scheduled"], false, "asked by a person");
    assert!(
        cut["records"].is_null() && cut["manifest"].is_null(),
        "not evaluated yet"
    );
    assert_eq!(
        f.get("/quality/datasets/cards/cuts").await.0,
        StatusCode::FORBIDDEN
    );

    // Its extraction fails for good: the scheduler's round marks the cut
    // failed, at extract, with what the job said.
    sqlx::query(
        "UPDATE quality.jobs SET state = 'failed', error = 'extract: the job came with no source'",
    )
    .execute(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(quality::firing::fail_cuts(&f.db.pool).await.unwrap(), 1);
    let (_, cuts) = f.get("/quality/datasets/loans/cuts").await;
    assert_eq!(
        (
            cuts[0]["state"].as_str(),
            cuts[0]["failed_at"].as_str(),
            cuts[0]["error"].as_str()
        ),
        (
            Some("failed"),
            Some("extract"),
            Some("extract: the job came with no source")
        )
    );
    assert_eq!(
        quality::firing::fail_cuts(&f.db.pool).await.unwrap(),
        0,
        "once"
    );
}
