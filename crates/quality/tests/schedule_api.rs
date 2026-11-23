//! The schedules' routes for people on an ephemeral PostgreSQL, driven
//! without a socket, with the `Caller` server would put in each request
//! (decision 0035): a schedule is saved only for a pipeline the caller may
//! schedule and a dataset the catalog has, with its next fire; each change
//! is audited; one with cuts is not deleted. Synthetic names only.

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

struct NoSealer;
impl Sealer for NoSealer {
    fn seal(&self, _: &str) -> Result<String, String> {
        Err("unused".into())
    }
    fn open(&self, _: &str) -> Result<String, String> {
        Err("unused".into())
    }
}

/// Sees every pipeline; schedules `loans` only, or every one (`*`) too.
struct Steward {
    everything: bool,
}
impl Access for Steward {
    fn can(&self, action: &str, resource: &str) -> bool {
        match action {
            "quality:ViewPipelines" => true,
            "quality:ManageSchedules" => resource == "pipeline/loans" || self.everything,
            _ => false,
        }
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

/// Has `loans` and `cards`.
struct TwoDatasets;
#[async_trait::async_trait]
impl Catalog for TwoDatasets {
    async fn snapshot(&self, _: Uuid, dataset: &str) -> Result<Option<CatalogSnapshot>, String> {
        Ok(["loans", "cards"]
            .contains(&dataset)
            .then(|| CatalogSnapshot {
                commit: "c1".into(),
                mapping: String::new(),
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
        "ALTER TABLE quality.schedules OWNER TO app",
        "ALTER TABLE quality.cuts OWNER TO app",
    ] {
        sqlx::query(sql).execute(&db.pool).await.unwrap();
    }
    let app = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    let router = QualityModule::default()
        .with_sources(app, Arc::new(NoSealer), Some(Arc::new(TwoDatasets)))
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
        everything: bool,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let caller = Caller::new(
            "00000000-0000-0000-0000-0000000000a1".into(),
            None,
            "192.0.2.1".into(),
            None,
            Arc::new(Steward { everything }),
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

fn daily(dataset: &str) -> Value {
    json!({ "cron": "0 6 * * *", "fires": "cut", "dataset": dataset })
}

#[tokio::test]
async fn a_steward_creates_edits_and_deletes_a_schedule_audited() {
    let f = fixture().await;
    let (status, made) = f
        .call(false, "POST", "/quality/schedules", Some(daily("loans")))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        (made["timezone"].as_str(), made["rerun"].as_str()),
        (Some("UTC"), Some("block")),
        "without the platform's time zone, UTC"
    );
    assert!(made["next_fire_at"].is_string(), "its first fire");
    let uri = format!("/quality/schedules/{}", made["id"].as_str().unwrap());

    let mut edit = daily("loans");
    edit["rerun"] = json!("replace");
    edit["enabled"] = json!(false);
    let (status, edited) = f.call(false, "PUT", &uri, Some(edit)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (edited["rerun"].as_str(), edited["enabled"].as_bool()),
        (Some("replace"), Some(false))
    );

    let (status, listed) = f.call(false, "GET", "/quality/schedules", None).await;
    assert_eq!(
        (status, listed.as_array().unwrap().len()),
        (StatusCode::OK, 1)
    );

    assert_eq!(
        f.call(false, "DELETE", &uri, None).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.call(false, "DELETE", &uri, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.changes(),
        [Change::Created, Change::Changed, Change::Deleted]
    );
    let resource = &f.log.0.lock().unwrap()[0].resource;
    assert!(
        resource.starts_with("pipeline/loans/schedule/"),
        "{resource}"
    );
}

#[tokio::test]
async fn a_schedule_out_of_reach_or_that_does_not_hold_is_refused_unaudited() {
    let f = fixture().await;
    let maintain = json!({ "cron": "0 2 * * 0", "fires": "job", "kind": "maintain" });
    // Another pipeline, or the whole warehouse: not this steward's.
    for body in [daily("cards"), maintain.clone()] {
        assert_eq!(
            f.call(false, "POST", "/quality/schedules", Some(body))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    for (body, code) in [
        (
            json!({ "cron": "0 6 * *", "fires": "cut", "dataset": "loans" }),
            "fields",
        ),
        (
            json!({ "cron": "0 6 * * *", "timezone": "Mars/Base", "fires": "cut", "dataset": "loans" }),
            "timezone",
        ),
        (
            json!({
                "trigger": { "start": "2026-11-02T06:00:00",
                             "recurrence": { "kind": "weekly", "every": 1, "days": [] } },
                "fires": "cut", "dataset": "loans",
            }),
            "trigger",
        ),
        (
            json!({ "cron": "0 6 * * *", "fires": "cut", "dataset": "loans", "rerun": "often" }),
            "rerun",
        ),
        (
            json!({ "cron": "0 6 * * *", "fires": "now", "dataset": "loans" }),
            "fires",
        ),
        (
            json!({ "cron": "0 6 * * *", "fires": "job", "kind": "evaluate" }),
            "kind",
        ),
        (
            json!({ "cron": "0 6 * * *", "fires": "job", "kind": "map" }),
            "dataset",
        ),
        (
            json!({ "cron": "0 6 * * *", "fires": "cut", "dataset": "ghost" }),
            "unknown_dataset",
        ),
    ] {
        let (status, out) = f.call(true, "POST", "/quality/schedules", Some(body)).await;
        assert_eq!(
            (status, out["error"].as_str()),
            (StatusCode::UNPROCESSABLE_ENTITY, Some(code))
        );
    }
    assert!(f.changes().is_empty(), "nothing done, nothing audited");

    // A maintenance is a dataset's (decision 0039): not the whole warehouse.
    let (status, out) = f
        .call(true, "POST", "/quality/schedules", Some(maintain))
        .await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("dataset"))
    );
    let loans =
        json!({ "cron": "0 2 * * 0", "fires": "job", "kind": "maintain", "dataset": "loans" });
    assert_eq!(
        f.call(false, "POST", "/quality/schedules", Some(loans))
            .await
            .0,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn a_schedule_with_cuts_is_disabled_not_deleted() {
    let f = fixture().await;
    let (_, made) = f
        .call(false, "POST", "/quality/schedules", Some(daily("loans")))
        .await;
    let id = Uuid::parse_str(made["id"].as_str().unwrap()).unwrap();
    sqlx::query(
        "INSERT INTO quality.cuts (tenant_id, dataset, cut, schedule_id, due_at)
         VALUES ($1::uuid, 'loans', '2026-10-31', $2, now())",
    )
    .bind(tenancy::MAIN_ID)
    .bind(id)
    .execute(&f.db.pool)
    .await
    .unwrap();
    let (status, out) = f
        .call(false, "DELETE", &format!("/quality/schedules/{id}"), None)
        .await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::CONFLICT, Some("has_cuts"))
    );
}

#[tokio::test]
async fn a_schedule_without_a_time_zone_takes_the_platforms() {
    let f = fixture().await;
    sqlx::query(
        "INSERT INTO system_settings (section, data) VALUES ('platform', '{\"timezone\": \"Europe/Madrid\"}')",
    )
    .execute(&f.db.pool)
    .await
    .unwrap();
    let (status, made) = f
        .call(false, "POST", "/quality/schedules", Some(daily("loans")))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(made["timezone"], "Europe/Madrid");

    let mut own = daily("loans");
    own["timezone"] = json!("America/Bogota");
    let (status, made) = f.call(false, "POST", "/quality/schedules", Some(own)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(made["timezone"], "America/Bogota", "one it names, it keeps");
}
