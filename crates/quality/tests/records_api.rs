//! A source table's records for people (decision 0049, points 3 to 5), on
//! an ephemeral PostgreSQL, driven without a socket with the `Caller`
//! server would put in each request: a page asked of a table the last
//! listing has is queued as a `records` job; what cannot be asked is
//! refused before; the page is read only by whoever asked, its values
//! masked on a read and revealed with a reason behind
//! `quality:ViewPersonalData` on every dataset that reads the table
//! (decisions 0064, 0069), each reveal audited with its reason and shape;
//! a worker's page of another table is refused. Synthetic names and values
//! only.

pub mod worker;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use contracts::{
    Access, AuditEntry, AuditTrail, Caller, Change, Module, ModuleRegistry, ViewDetail, ViewPurpose,
};
use infrastructure::module_migrations::PgModuleMigrator;
use quality::catalog::{Catalog, CatalogSnapshot, DatasetMapping};
use quality::module::QualityModule;
use quality::source_store::Sealer;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use uuid::Uuid;

const ANA: &str = "00000000-0000-0000-0000-0000000000a1";
const BEN: &str = "00000000-0000-0000-0000-0000000000b2";
const MAIN: &str = "00000000-0000-0000-0000-000000000001";

struct Plain;
impl Sealer for Plain {
    fn seal(&self, plain: &str) -> Result<String, String> {
        Ok(plain.to_string())
    }
    fn open(&self, sealed: &str) -> Result<String, String> {
        Ok(sealed.to_string())
    }
}

/// May view the source `core`; sees personal data on the datasets
/// `personal` names (`*`: on `pipeline/*`), never on those `denied` names.
#[derive(Clone, Copy)]
struct Viewer {
    personal: &'static [&'static str],
    denied: &'static [&'static str],
}
const NONE: Viewer = Viewer {
    personal: &[],
    denied: &[],
};
const EVERY: Viewer = Viewer {
    personal: &["*"],
    denied: &[],
};
impl Access for Viewer {
    fn can(&self, action: &str, resource: &str) -> bool {
        match (action, resource.split_once('/')) {
            ("quality:ViewSources", _) => resource == "source/core",
            ("quality:ViewPersonalData", Some(("pipeline", d))) => {
                self.personal.iter().any(|p| *p == "*" || *p == d) && !self.denied.contains(&d)
            }
            _ => false,
        }
    }
    fn can_some(&self, action: &str) -> bool {
        self.can(action, "source/core")
    }
    fn can_every(&self, action: &str, kind: &str) -> bool {
        (action, kind) == ("quality:ViewPersonalData", "pipeline")
            && self.personal.contains(&"*")
            && self.denied.is_empty()
    }
}

/// The catalog's `main`: `credits` and `savings` read `core`'s
/// `dbo.clients`, `loans` another table, `branch` another source.
struct Mappings;
#[async_trait::async_trait]
impl Catalog for Mappings {
    async fn snapshot(&self, _: Uuid, _: &str) -> Result<Option<CatalogSnapshot>, String> {
        Ok(None)
    }
    async fn mappings(&self, _: Uuid) -> Result<Vec<DatasetMapping>, String> {
        let read = |dataset: &str, source: &str, table: &str| {
            DatasetMapping {
            dataset: dataset.into(),
            yaml: format!(
                "dataset: {dataset}\nsource: {dataset}\nfrom: {{ source: {source}, table: {table} }}\n\
                 key: [id]\nfields:\n  id: {{ column: ID, type: text }}\n"
            ),
        }
        };
        Ok(vec![
            read("credits", "core", "dbo.clients"),
            read("savings", "core", "dbo.clients"),
            read("loans", "core", "dbo.loans"),
            read("branch", "branch", "dbo.accounts"),
        ])
    }
}

/// The audit: entries, views with their detail, and whether it refuses.
#[derive(Default)]
struct Log(Mutex<Vec<AuditEntry>>, Mutex<Vec<ViewDetail>>, AtomicBool);
#[async_trait::async_trait]
impl AuditTrail for Log {
    async fn record(&self, _: &Caller, entry: &AuditEntry) -> Result<(), String> {
        self.0.lock().unwrap().push(entry.clone());
        Ok(())
    }
    async fn record_view(
        &self,
        _: &Caller,
        entry: &AuditEntry,
        detail: &ViewDetail,
    ) -> Result<(), String> {
        if self.2.load(Ordering::SeqCst) {
            return Err("the log is down".into());
        }
        self.0.lock().unwrap().push(entry.clone());
        self.1.lock().unwrap().push(detail.clone());
        Ok(())
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
    let columns = json!([
        { "name": "id", "data_type": "integer" }, { "name": "doc", "data_type": "varchar" }
    ]);
    let tables = json!([{ "schema": "dbo", "name": "clients", "columns": columns },
        { "schema": "dbo", "name": "accounts", "columns": columns }]);
    for sql in [
        "GRANT USAGE ON SCHEMA quality TO app",
        "ALTER TABLE quality.jobs OWNER TO app",
        "ALTER TABLE quality.sources OWNER TO app",
        "ALTER TABLE quality.source_tables OWNER TO app",
        "ALTER TABLE quality.source_records OWNER TO app",
        "INSERT INTO quality.sources (tenant_id, name, driver, fields, secrets)
         VALUES ('00000000-0000-0000-0000-000000000001', 'core', 'sqlserver', '{}', '{}')",
        "INSERT INTO quality.jobs (tenant_id, id, kind, parameters, state)
         VALUES ('00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-0000000000f1',
                 'tables', '{\"source\": \"core\"}', 'done')",
    ] {
        sqlx::query(sql).execute(&db.pool).await.unwrap();
    }
    sqlx::query(
        "INSERT INTO quality.source_tables (tenant_id, source, job, asked_at, tables)
         VALUES ($1::uuid, 'core', '00000000-0000-0000-0000-0000000000f1', now(), $2)",
    )
    .bind(MAIN)
    .bind(tables)
    .execute(&db.pool)
    .await
    .unwrap();
    let app = infrastructure::db::with_tenancy(PgPoolOptions::new(), tenancy::Mode::Single)
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    let router = QualityModule::default()
        .with_sources(app, Arc::new(Plain), Some(Arc::new(Mappings)))
        .user_router();
    Fixture {
        db,
        router,
        log: Arc::default(),
    }
}

impl Fixture {
    async fn call(
        &self,
        who: &str,
        viewer: Viewer,
        method: &str,
        uri: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let caller = Caller::new(
            who.into(),
            None,
            "192.0.2.1".into(),
            None,
            Arc::new(viewer),
            self.log.clone(),
        );
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
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

    async fn ask(&self, body: Value) -> (StatusCode, Value) {
        self.call(ANA, NONE, "POST", "/quality/sources/core/records", body)
            .await
    }

    /// Whether `viewer` may reveal the page at `uri`, as its read says.
    async fn revealable(&self, viewer: Viewer, uri: &str) -> Value {
        let (_, out) = self.call(ANA, viewer, "GET", uri, Value::Null).await;
        assert_eq!(out["masked"], true, "a read is always masked");
        out["revealable"].clone()
    }

    /// `viewer`'s reveal of the page at `uri` with `reason`.
    async fn reveal(&self, viewer: Viewer, uri: &str, reason: Value) -> (StatusCode, Value) {
        self.call(ANA, viewer, "POST", &format!("{uri}/reveal"), reason)
            .await
    }

    fn views(&self) -> usize {
        self.log.1.lock().unwrap().len()
    }

    /// The worker's part, as its report would leave it.
    async fn read(&self, job: &str, table: &str) {
        let page = json!({ "source": "core", "table": table, "columns": ["id", "doc"],
            "rows": [["1", "100200300"], ["2", null]], "more": false });
        let job = Uuid::parse_str(job).unwrap();
        sqlx::query("UPDATE quality.jobs SET state = 'done' WHERE id = $1")
            .bind(job)
            .execute(&self.db.pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO quality.source_records (tenant_id, job, page) VALUES ($1::uuid, $2, $3)",
        )
        .bind(MAIN)
        .bind(job)
        .bind(page)
        .execute(&self.db.pool)
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn a_page_is_queued_and_what_cannot_be_asked_is_refused() {
    let f = fixture().await;
    let last = json!({ "table": "dbo.clients", "order": "id", "descending": true });
    let (status, out) = f.ask(last).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let job = Uuid::parse_str(out["job"].as_str().unwrap()).unwrap();
    let (kind, parameters): (String, Value) =
        sqlx::query_as("SELECT kind, parameters FROM quality.jobs WHERE id = $1")
            .bind(job)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert_eq!(kind, "records");
    assert_eq!(
        (parameters["source"].as_str(), parameters["by"].as_str()),
        (Some("core"), Some(ANA))
    );

    for (body, why) in [
        (json!({ "table": "dbo.nope" }), "table"),
        (json!({ "table": "clients" }), "table"),
        (json!({ "table": "dbo.clients", "order": "nope" }), "order"),
        (
            json!({ "table": "dbo.clients", "descending": true }),
            "no_order",
        ),
        (json!({ "table": "dbo.clients", "page": 1000 }), "too_far"),
    ] {
        let (status, out) = f.ask(body).await;
        assert_eq!(
            (status, out["error"].as_str()),
            (StatusCode::UNPROCESSABLE_ENTITY, Some(why))
        );
    }
    let other = f
        .call(
            ANA,
            NONE,
            "POST",
            "/quality/sources/other/records",
            json!({ "table": "a.b" }),
        )
        .await;
    assert_eq!(other.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_page_is_read_masked_by_everyone_and_audits_nothing() {
    let f = fixture().await;
    let (_, out) = f.ask(json!({ "table": "dbo.clients", "page": 2 })).await;
    let job = out["job"].as_str().unwrap().to_string();
    let uri = format!("/quality/sources/core/records/{job}");

    let (status, out) = f.call(ANA, EVERY, "GET", &uri, Value::Null).await;
    assert_eq!(
        (status, out["state"].as_str()),
        (StatusCode::OK, Some("queued"))
    );
    assert!(out["page"].is_null());
    assert_eq!(out["revealable"], false, "nothing to show yet");

    f.read(&job, "dbo.clients").await;
    for viewer in [NONE, EVERY, EVERY, EVERY] {
        let (_, out) = f.call(ANA, viewer, "GET", &uri, Value::Null).await;
        assert_eq!(out["masked"], true);
        assert_eq!(out["page"]["rows"][0], json!(["••••", "••••0300"]));
        assert!(out["page"]["rows"][1][1].is_null(), "empty stays empty");
    }
    assert_eq!(f.revealable(NONE, &uri).await, false);
    assert_eq!(f.revealable(EVERY, &uri).await, true);
    assert!(f.log.0.lock().unwrap().is_empty(), "polling writes no row");

    let (status, _) = f.call(BEN, EVERY, "GET", &uri, Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "only whoever asked");
}

#[tokio::test]
async fn values_need_the_permission_on_every_dataset_that_reads_the_table() {
    let f = fixture().await;
    let mut pages = Vec::new();
    for table in ["dbo.clients", "dbo.accounts"] {
        let (_, out) = f.ask(json!({ "table": table })).await;
        let job = out["job"].as_str().unwrap().to_string();
        f.read(&job, table).await;
        pages.push(format!("/quality/sources/core/records/{job}"));
    }
    let (clients, accounts) = (pages[0].as_str(), pages[1].as_str());
    let readers = Viewer {
        personal: &["credits", "savings"],
        denied: &[],
    };
    assert_eq!(f.revealable(readers, clients).await, true, "both readers");
    let one = Viewer {
        personal: &["credits", "loans"],
        denied: &[],
    };
    assert_eq!(f.revealable(one, clients).await, false, "savings missing");
    let denied = Viewer {
        personal: &["*"],
        denied: &["savings"],
    };
    assert_eq!(f.revealable(denied, clients).await, false, "savings denied");
    assert_eq!(
        f.revealable(denied, accounts).await,
        false,
        "no reader: all"
    );
    assert_eq!(
        f.revealable(readers, accounts).await,
        false,
        "no reader: all"
    );
    assert_eq!(f.revealable(EVERY, accounts).await, true);
    let reason = json!({ "purpose": "quality_review" });
    assert_eq!(
        f.reveal(one, clients, reason.clone()).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(f.reveal(readers, clients, reason).await.0, StatusCode::OK);
    assert_eq!(f.views(), 1, "only the reveal answered");
}

#[tokio::test]
async fn a_reveal_answers_the_values_after_one_row_with_its_reason_and_shape() {
    let f = fixture().await;
    let (_, out) = f.ask(json!({ "table": "dbo.clients", "page": 2 })).await;
    let job = out["job"].as_str().unwrap().to_string();
    let uri = format!("/quality/sources/core/records/{job}");
    f.read(&job, "dbo.clients").await;

    let reason = json!({ "purpose": "client_request", "reference": "INC-2041" });
    let (status, out) = f.reveal(EVERY, &uri, reason.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (&out["state"], &out["masked"]),
        (&json!("done"), &json!(false))
    );
    assert_eq!(out["page"]["rows"][0], json!(["1", "100200300"]));
    {
        let log = f.log.0.lock().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].resource, "source/core/dbo.clients/page/2");
        assert_eq!(log[0].change, Change::Viewed);
        let view = &f.log.1.lock().unwrap()[0];
        assert_eq!(view.reason.purpose(), ViewPurpose::ClientRequest);
        assert_eq!(view.reason.reference().unwrap().as_str(), "INC-2041");
        assert_eq!((view.job.as_str(), view.rows_shown), (job.as_str(), 2));
        assert_eq!(view.fields_shown, ["id", "doc"]);
    }
    let (status, _) = f.reveal(EVERY, &uri, reason).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(f.views(), 2, "a second showing, a second row");
}

#[tokio::test]
async fn a_refused_reveal_writes_no_row_and_shows_nothing() {
    let f = fixture().await;
    let (_, out) = f.ask(json!({ "table": "dbo.clients" })).await;
    let job = out["job"].as_str().unwrap().to_string();
    let uri = format!("/quality/sources/core/records/{job}");
    let fine = json!({ "purpose": "audit" });
    let refused = |(status, out): (StatusCode, Value)| (status, out["error"].clone());

    let not_done = f.reveal(EVERY, &uri, fine.clone()).await;
    assert_eq!(refused(not_done), (StatusCode::CONFLICT, json!("not_done")));
    f.read(&job, "dbo.clients").await;
    for (reason, code) in [
        (json!({ "purpose": "curiosity" }), "purpose"),
        (json!({ "reference": "INC-2041" }), "purpose"),
        (
            json!({ "purpose": "audit", "reference": "12345678" }),
            "reference",
        ),
        (json!({ "purpose": "other" }), "reference"),
        (json!({ "purpose": "other", "reference": "" }), "reference"),
    ] {
        let out = refused(f.reveal(EVERY, &uri, reason).await);
        assert_eq!(out, (StatusCode::UNPROCESSABLE_ENTITY, json!(code)));
    }
    let out = refused(f.reveal(NONE, &uri, fine.clone()).await);
    assert_eq!(out, (StatusCode::FORBIDDEN, json!("forbidden")));
    let theirs = f
        .call(BEN, EVERY, "POST", &format!("{uri}/reveal"), fine.clone())
        .await;
    assert_eq!(theirs.0, StatusCode::NOT_FOUND, "another person's");
    f.log.2.store(true, Ordering::SeqCst);
    let (status, out) = f.reveal(EVERY, &uri, fine.clone()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(out, json!({ "error": "audit_unavailable" }), "no values");
    assert!(f.log.0.lock().unwrap().is_empty(), "no row for a refusal");

    f.log.2.store(false, Ordering::SeqCst);
    sqlx::query("DELETE FROM quality.source_records")
        .execute(&f.db.pool)
        .await
        .unwrap();
    let purged = refused(f.reveal(EVERY, &uri, fine).await);
    assert_eq!(purged, (StatusCode::NOT_FOUND, json!("not_found")));
    assert_eq!(f.views(), 0);
}

#[tokio::test]
async fn a_worker_gets_the_source_and_its_page_of_another_table_is_refused() {
    let f = worker::fixture().await;
    tenancy::within(worker::tenant(worker::B), async {
        let source = quality::source::Source {
            name: "core".into(),
            driver: "sqlserver".into(),
            fields: [("host", "db.example"), ("database", "core")]
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .into(),
            secrets: [("user", "reader"), ("password", "synthetic")]
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .into(),
        };
        quality::source_store::put(&f.app, &worker::Mirror, &source)
            .await
            .unwrap();
    })
    .await;
    let job: Uuid = sqlx::query_scalar(
        "INSERT INTO quality.jobs (tenant_id, kind, parameters, state, worker, lease_until)
         VALUES ($1::uuid, 'records', '{\"source\": \"core\", \"table\": \"dbo.clients\"}',
                 'running', 'w1/0', now() + interval '1 minute') RETURNING id",
    )
    .bind(worker::B)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    let uri = format!("/internal/jobs/{}/{job}", worker::B);
    let (status, body) = f.call("GET", &uri, "token-w1", None).await;
    assert_eq!(status, StatusCode::OK);
    let details: quality::worker_api::JobDetails = serde_json::from_slice(&body).unwrap();
    let source = details.source.unwrap();
    assert_eq!(source.table, "dbo.clients");
    assert!(
        source.on_trino() && source.secrets.is_empty(),
        "Trino reads it, with no credential"
    );

    let report = format!("{uri}/report");
    let page = |table: &str| json!({ "source": "core", "table": table, "columns": ["id"], "rows": [["1"]], "more": false });
    let other = f
        .call("POST", &report, "token-w1", Some(page("dbo.other")))
        .await;
    assert_eq!(other.0, StatusCode::UNPROCESSABLE_ENTITY);
    let kept = f
        .call("POST", &report, "token-w1", Some(page("dbo.clients")))
        .await;
    assert_eq!(kept.0, StatusCode::NO_CONTENT);
}
