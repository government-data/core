//! A rule tested as it is written (decisions 0019 point 10, 0043), on an
//! ephemeral PostgreSQL, driven without a socket with the `Caller` server
//! would put in each request: a rules file as typed is queued as a
//! `test_rule` job on the dataset's latest cut; a text that does not lint
//! is refused before; its result is read only by whoever asked, masked,
//! and revealed with a reason behind `quality:ViewPersonalData`, each
//! reveal audited with its reason and shape (decision 0069).
//! Synthetic names and values only.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use contracts::{
    Access, AuditEntry, AuditTrail, Caller, Change, Module, ModuleRegistry, ViewDetail, ViewPurpose,
};
use infrastructure::module_migrations::PgModuleMigrator;
use quality::draft_api::{Approval, DraftCatalog, DraftFiles, DraftInfo, FileEdit, FileSaved};
use quality::module::QualityModule;
use quality::source_store::Sealer;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use uuid::Uuid;

const MAPPING: &str =
    "dataset: loans\nsource: loans\nkey: [Id]\nfields:\n  Id: { column: id, type: text }\n";
const GOOD: &str = "dataset: loans\nrules:\n  - id: L-01\n    field: Id\n    check: required\n    severity: major\n    cause: capture\n    alert: Empty\n";
const BAD: &str = "dataset: loans\nrules:\n  - id: L-01\n    field: Nope\n    check: required\n    severity: major\n    cause: capture\n    alert: Empty\n";
const ANA: &str = "00000000-0000-0000-0000-0000000000a1";
const BEN: &str = "00000000-0000-0000-0000-0000000000b2";

struct NoSealer;
impl Sealer for NoSealer {
    fn seal(&self, _: &str) -> Result<String, String> {
        Err("unused".into())
    }
    fn open(&self, _: &str) -> Result<String, String> {
        Err("unused".into())
    }
}

/// `main` has `loans` and `cards`; nothing written here.
struct Main;
#[async_trait::async_trait]
impl DraftCatalog for Main {
    async fn list(&self, _: &str) -> Result<Vec<DraftInfo>, String> {
        Ok(Vec::new())
    }
    async fn read(&self, _: &str, dataset: &str) -> Result<Option<DraftFiles>, String> {
        Ok(matches!(dataset, "loans" | "cards").then(|| DraftFiles {
            commit: "c1".into(),
            mapping: MAPPING.replace("loans", dataset),
            rules: Vec::new(),
        }))
    }
    async fn approve(&self, _: &str, _: &str, _: &str) -> Result<Approval, String> {
        unreachable!("nothing here writes")
    }
    async fn discard(&self, _: &str) -> Result<bool, String> {
        unreachable!("nothing here writes")
    }
    async fn save_file(&self, _: &FileEdit<'_>, _: bool) -> Result<FileSaved, String> {
        unreachable!("nothing here writes")
    }
}

/// May view the rules of `loans` and `cards`; sees personal data if `personal`.
struct Analyst {
    personal: bool,
}
impl Access for Analyst {
    fn can(&self, action: &str, resource: &str) -> bool {
        match action {
            "quality:ViewRules" => matches!(resource, "catalog/loans" | "catalog/cards"),
            "quality:ViewPersonalData" => self.personal && resource == "pipeline/loans",
            _ => false,
        }
    }
    fn can_some(&self, action: &str) -> bool {
        self.can(action, "catalog/loans")
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
    for sql in [
        "GRANT USAGE ON SCHEMA quality TO app",
        "ALTER TABLE quality.jobs OWNER TO app",
        "ALTER TABLE quality.cuts OWNER TO app",
        "ALTER TABLE quality.rule_tests OWNER TO app",
        "ALTER TABLE quality.run_summary OWNER TO app",
        // `loans` has a cut extracted; `cards` none.
        "INSERT INTO quality.cuts (tenant_id, dataset, cut, state, bronze_snapshot, due_at)
         VALUES ('00000000-0000-0000-0000-000000000001', 'loans', '2026-10-31', 'done', '42', now())",
    ] {
        sqlx::query(sql).execute(&db.pool).await.unwrap();
    }
    let app = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    let router = QualityModule::default()
        .with_sources(app, Arc::new(NoSealer), None)
        .with_drafts(Arc::new(Main))
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
        personal: bool,
        method: &str,
        uri: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let caller = Caller::new(
            who.into(),
            None,
            "192.0.2.1".into(),
            None,
            Arc::new(Analyst { personal }),
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

    async fn start(&self, dataset: &str, yaml: &str) -> (StatusCode, Value) {
        self.start_showing(dataset, yaml, &[]).await
    }

    /// A test that asks to show `columns` too.
    async fn start_showing(
        &self,
        dataset: &str,
        yaml: &str,
        columns: &[&str],
    ) -> (StatusCode, Value) {
        let body = json!({ "dataset": dataset, "yaml": yaml, "rule": "L-01", "columns": columns });
        self.call(ANA, false, "POST", "/quality/rules/test", body)
            .await
    }

    /// The worker's part, as its report would leave it.
    async fn done(&self, job: &str) {
        let report = json!({ "cut": "2026-10-31", "records": 3, "rules": [{
            "rule": "L-01", "applies": 2, "passes": 1, "fails": 1, "skips": 1,
            "records": [
                { "outcome": "fails", "values": [{ "field": "Id", "value": "ID-123456" }] },
                { "outcome": "passes", "values": [{ "field": "Id", "value": "ID-1" }] },
                { "outcome": "skips", "values": [{ "field": "Id", "value": null }] }
            ],
            "groups": [{ "shape": "AA-999999", "count": 1, "records": [
                { "outcome": "fails", "values": [{ "field": "Id", "value": "ID-123456" }] }
            ] }]
        }]});
        let job = Uuid::parse_str(job).unwrap();
        sqlx::query("UPDATE quality.jobs SET state = 'done' WHERE id = $1")
            .bind(job)
            .execute(&self.db.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO quality.rule_tests (tenant_id, job, report) VALUES ('00000000-0000-0000-0000-000000000001', $1, $2)")
            .bind(job)
            .bind(report)
            .execute(&self.db.pool)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_rules_file_as_typed_is_queued_on_the_latest_cut() {
    let f = fixture().await;
    let (status, out) = f.start("loans", GOOD).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{out}");
    assert_eq!(out["cut"], "2026-10-31");
    let (kind, parameters, priority): (String, Value, i32) =
        sqlx::query_as("SELECT kind, parameters, priority FROM quality.jobs WHERE id = $1::uuid")
            .bind(out["job"].as_str().unwrap())
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert_eq!(kind, "test_rule");
    assert_eq!(parameters["yaml"], GOOD);
    assert_eq!(parameters["snapshot"], "42");
    assert_eq!(parameters["rule"], "L-01");
    assert_eq!(parameters["by"], ANA);
    assert_eq!(parameters["columns"], json!([]));
    assert!(priority > 0, "before the scheduled work");

    // Fields of the mapping to show too; none it does not have.
    let (status, out) = f.start_showing("loans", GOOD, &["Id"]).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{out}");
    let (parameters,): (Value,) =
        sqlx::query_as("SELECT parameters FROM quality.jobs WHERE id = $1::uuid")
            .bind(out["job"].as_str().unwrap())
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert_eq!(parameters["columns"], json!(["Id"]));
    let (status, out) = f.start_showing("loans", GOOD, &["Nope"]).await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("columns"))
    );
}

#[tokio::test]
async fn a_cut_loaded_by_hand_and_evaluated_is_tested_as_it_is() {
    let f = fixture().await;
    sqlx::query(
        "INSERT INTO quality.run_summary (tenant_id, dataset, cut, job_id, records, duration_ms, rules_commit)
         VALUES ('00000000-0000-0000-0000-000000000001', 'cards', '2021-03-10', gen_random_uuid(), 3, 1, 'c1')",
    )
    .execute(&f.db.pool)
    .await
    .unwrap();
    let (status, out) = f.start("cards", &GOOD.replace("loans", "cards")).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{out}");
    assert_eq!(out["cut"], "2021-03-10");
    let (parameters,): (Value,) =
        sqlx::query_as("SELECT parameters FROM quality.jobs WHERE id = $1::uuid")
            .bind(out["job"].as_str().unwrap())
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert!(
        parameters["snapshot"].is_null(),
        "no snapshot: as it is now"
    );
}

#[tokio::test]
async fn what_does_not_lint_or_has_no_cut_is_not_queued() {
    let f = fixture().await;
    let (status, out) = f.start("loans", BAD).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(out["error"], "lint");
    let (status, out) = f.start("cards", &GOOD.replace("loans", "cards")).await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::CONFLICT, Some("no_cut"))
    );
    assert_eq!(f.start("other", GOOD).await.0, StatusCode::FORBIDDEN);
    let (jobs,): (i64,) = sqlx::query_as("SELECT count(*) FROM quality.jobs")
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    assert_eq!(jobs, 0);
}

#[tokio::test]
async fn its_records_are_read_masked_by_everyone_and_audit_nothing() {
    let f = fixture().await;
    let (_, out) = f.start("loans", GOOD).await;
    let job = out["job"].as_str().unwrap().to_string();
    let uri = format!("/quality/rules/tests/{job}");

    let (status, out) = f.call(ANA, true, "GET", &uri, Value::Null).await;
    assert_eq!(
        (status, out["state"].as_str()),
        (StatusCode::OK, Some("queued"))
    );
    assert!(out["report"].is_null());
    assert_eq!(out["revealable"], false, "nothing to show yet");

    f.done(&job).await;
    for personal in [false, true, true] {
        let (_, out) = f.call(ANA, personal, "GET", &uri, Value::Null).await;
        assert_eq!(out["masked"], true);
        assert_eq!(out["revealable"], personal);
        let records = &out["report"]["rules"][0]["records"];
        assert_eq!(records[0]["values"][0]["value"], "••••3456");
        assert_eq!(records[1]["values"][0]["value"], "••••");
        assert!(
            records[2]["values"][0]["value"].is_null(),
            "empty stays empty"
        );
        // A group's shape says no value; its records are masked as the others.
        let group = &out["report"]["rules"][0]["groups"][0];
        assert_eq!(
            (&group["shape"], &group["count"]),
            (&json!("AA-999999"), &json!(1))
        );
        assert_eq!(group["records"][0]["values"][0]["value"], "••••3456");
    }
    assert!(f.log.0.lock().unwrap().is_empty(), "polling writes no row");

    let (status, _) = f.call(BEN, true, "GET", &uri, Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "only whoever asked");
}

#[tokio::test]
async fn a_done_test_whose_report_is_gone_or_unreadable_is_expired_never_running() {
    let f = fixture().await;
    let state = |job: Uuid, state: &'static str| {
        let pool = f.db.pool.clone();
        async move {
            sqlx::query("UPDATE quality.jobs SET state = $2 WHERE id = $1")
                .bind(job)
                .bind(state)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    let read = |job: Uuid| {
        let f = &f;
        async move {
            let uri = format!("/quality/rules/tests/{job}");
            f.call(ANA, true, "GET", &uri, Value::Null).await
        }
    };
    let mut jobs = Vec::new();
    for _ in 0..3 {
        let (_, out) = f.start("loans", GOOD).await;
        jobs.push(Uuid::parse_str(out["job"].as_str().unwrap()).unwrap());
    }
    let [running, purged, unreadable] = jobs[..] else {
        unreachable!()
    };

    state(running, "running").await;
    let (status, out) = read(running).await;
    assert_eq!((status, &out["state"]), (StatusCode::OK, &json!("running")));
    f.done(&running.to_string()).await;
    assert_eq!(read(running).await.1["state"], "done", "its report kept");

    // Purged after a day: the job stays done, its report row is gone.
    state(purged, "done").await;
    sqlx::query("INSERT INTO quality.rule_tests (tenant_id, job, report) VALUES ('00000000-0000-0000-0000-000000000001', $1, '{\"nope\": 1}')")
        .bind(unreadable)
        .execute(&f.db.pool)
        .await
        .unwrap();
    state(unreadable, "done").await;
    for job in [purged, unreadable] {
        let (status, out) = read(job).await;
        assert_eq!((status, &out["state"]), (StatusCode::OK, &json!("expired")));
        assert!(out["report"].is_null(), "{out}");
        assert_eq!(out["revealable"], false);
    }
}

#[tokio::test]
async fn a_reveal_answers_the_records_after_one_row_with_its_reason_and_shape() {
    let f = fixture().await;
    let (_, out) = f.start("loans", GOOD).await;
    let job = out["job"].as_str().unwrap().to_string();
    let reveal = format!("/quality/rules/tests/{job}/reveal");
    f.done(&job).await;

    let reason = json!({ "purpose": "rule_writing", "reference": "CASO/2026/118" });
    let (status, out) = f.call(ANA, true, "POST", &reveal, reason.clone()).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(
        (&out["state"], &out["masked"]),
        (&json!("done"), &json!(false))
    );
    let rule = &out["report"]["rules"][0];
    assert_eq!(rule["records"][0]["values"][0]["value"], "ID-123456");
    assert_eq!(
        rule["groups"][0]["records"][0]["values"][0]["value"],
        "ID-123456"
    );
    {
        let log = f.log.0.lock().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].resource, format!("dataset/loans/test/{job}"));
        assert_eq!(log[0].change, Change::Viewed);
        let view = &f.log.1.lock().unwrap()[0];
        assert_eq!(view.reason.purpose(), ViewPurpose::RuleWriting);
        assert_eq!(view.reason.reference().unwrap().as_str(), "CASO/2026/118");
        assert_eq!(view.job, job);
        assert_eq!(view.rows_shown, 4, "three under the rule, one in its group");
        assert_eq!(view.fields_shown, ["Id"]);
    }
    let (status, _) = f.call(ANA, true, "POST", &reveal, reason).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(f.log.1.lock().unwrap().len(), 2, "a second showing");
}

#[tokio::test]
async fn a_refused_reveal_writes_no_row_and_shows_nothing() {
    let f = fixture().await;
    let (_, out) = f.start("loans", GOOD).await;
    let job = out["job"].as_str().unwrap().to_string();
    let reveal = format!("/quality/rules/tests/{job}/reveal");
    let fine = json!({ "purpose": "quality_review" });
    let post = |who: &'static str, personal: bool, body: Value| {
        let reveal = reveal.clone();
        let f = &f;
        async move {
            let (status, out) = f.call(who, personal, "POST", &reveal, body).await;
            (status, out["error"].clone())
        }
    };

    let not_done = post(ANA, true, fine.clone()).await;
    assert_eq!(not_done, (StatusCode::CONFLICT, json!("not_done")));
    f.done(&job).await;
    for (reason, code) in [
        (json!({ "purpose": "curiosity" }), "purpose"),
        (
            json!({ "purpose": "audit", "reference": "12345678" }),
            "reference",
        ),
        (json!({ "purpose": "other" }), "reference"),
    ] {
        let out = post(ANA, true, reason).await;
        assert_eq!(out, (StatusCode::UNPROCESSABLE_ENTITY, json!(code)));
    }
    let out = post(ANA, false, fine.clone()).await;
    assert_eq!(out, (StatusCode::FORBIDDEN, json!("forbidden")));
    let theirs = post(BEN, true, fine.clone()).await;
    assert_eq!(theirs.0, StatusCode::NOT_FOUND, "another person's");
    f.log.2.store(true, Ordering::SeqCst);
    let (status, out) = f.call(ANA, true, "POST", &reveal, fine).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(out, json!({ "error": "audit_unavailable" }), "no values");
    assert!(f.log.0.lock().unwrap().is_empty(), "no row for a refusal");
}
