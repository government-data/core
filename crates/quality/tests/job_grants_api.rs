//! The job grants route (decision 0090, point 7.2): with the rules merge's
//! machine token, the job users' grants filled from the catalog's `main`
//! (each dataset's source and its rules' references); without one, 401; a
//! failing catalog, 503. Driven without a socket. Synthetic names only.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use contracts::{Grantee, Grantees};
use quality::catalog::{Catalog, CatalogSnapshot, DatasetMapping, RulesSource};
use quality::job_grants_api::{routes, routes_with_people};
use quality::worker_api::{MachineCaller, MachineTokens};
use serde_json::Value;
use tower::ServiceExt;

const URI: &str = "/internal/quality/v1/job-grants";

struct Tokens;
impl MachineTokens for Tokens {
    fn client(&self, token: &str) -> Option<MachineCaller> {
        (token == "rules-merge").then(|| MachineCaller {
            client: "engine-trino".into(),
            machine: None,
        })
    }
}

/// `clients` reads source `core` and references `advisors`; `advisors`
/// has no source; `broken`'s mapping does not parse.
struct Main {
    fails: bool,
}

#[async_trait::async_trait]
impl Catalog for Main {
    async fn snapshot(&self, dataset: &str) -> Result<Option<CatalogSnapshot>, String> {
        let rules = match dataset {
            "clients" => vec![RulesSource {
                file: "clients.yaml".into(),
                yaml: "dataset: clients\nreferences:\n  advisors: { dataset: advisors, key: Code }\n\
                       rules:\n  - id: R-01\n    field: Kind\n    check: required\n    severity: major\n"
                    .into(),
            }],
            _ => Vec::new(),
        };
        Ok(Some(CatalogSnapshot {
            commit: "c1".into(),
            mapping: String::new(),
            rules,
            others: Vec::new(),
        }))
    }

    async fn mappings(&self) -> Result<Vec<DatasetMapping>, String> {
        if self.fails {
            return Err("down".into());
        }
        let m = |dataset: &str, yaml: &str| DatasetMapping {
            dataset: dataset.into(),
            yaml: yaml.into(),
        };
        Ok(vec![
            m(
                "clients",
                "dataset: clients\nsource: clients\nfrom: { source: core, table: public.clients }\n\
                 fields:\n  Kind: { column: kind, type: text }\n",
            ),
            m(
                "advisors",
                "dataset: advisors\nsource: advisors\nfields:\n  Code: { column: code, type: text }\n",
            ),
            m("broken", "dataset: [\n"),
        ])
    }
}

async fn ask(fails: bool, token: Option<&str>) -> (StatusCode, Value) {
    let router = routes(Arc::new(Main { fails }), Arc::new(Tokens));
    let mut req = Request::get(URI);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let res = router
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn subjects(body: &Value) -> Vec<String> {
    let mut s: Vec<String> = body["grants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| {
            format!(
                "{} {}",
                g["subject"].as_str().unwrap(),
                g["namespace"].as_str().unwrap()
            )
        })
        .collect();
    s.dedup();
    s
}

#[tokio::test]
async fn the_rules_merge_gets_the_grants_filled_from_main() {
    let (status, body) = ask(false, Some("rules-merge")).await;
    assert_eq!(status, StatusCode::OK);
    let s = subjects(&body);
    for want in [
        "job_rules_clients advisors_silver",
        "job_extract_clients src_core.*",
        "job_source_core src_core.*",
        "job_rules_advisors advisors_silver",
        "job_maint_gold quality_gold",
    ] {
        assert!(s.iter().any(|x| x == want), "{want} not in {s:?}");
    }
    assert!(s.iter().all(|x| !x.contains("broken")), "{s:?}");
    assert!(s
        .iter()
        .all(|x| !x.starts_with("job_extract_advisors src_")));
}

#[tokio::test]
async fn no_token_or_a_failing_catalog_gets_no_grants() {
    assert_eq!(ask(false, None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(ask(false, Some("other")).await.0, StatusCode::UNAUTHORIZED);
    let (status, body) = ask(true, Some("rules-merge")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "catalog_unavailable");
}

/// One person may view `clients`; with `fails`, the IAM does not answer.
struct People {
    fails: bool,
}

#[async_trait::async_trait]
impl Grantees for People {
    async fn allowed(&self, _: &str, resources: &[String]) -> Result<Vec<Grantee>, String> {
        if self.fails {
            return Err("down".into());
        }
        let clients = resources.iter().filter(|r| *r == "pipeline/clients");
        Ok(vec![Grantee {
            account: "acct-1".into(),
            resources: clients.cloned().collect(),
        }])
    }
}

async fn ask_with_people(fails: bool) -> (StatusCode, Value) {
    let router = routes_with_people(
        Arc::new(Main { fails: false }),
        Arc::new(Tokens),
        Arc::new(People { fails }),
    );
    let req = Request::get(URI).header("authorization", "Bearer rules-merge");
    let res = router
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn the_people_s_grants_come_beside_the_job_users() {
    let (status, body) = ask_with_people(false).await;
    assert_eq!(status, StatusCode::OK);
    let person: Vec<&Value> = body["grants"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|g| g["subject"] == "person-acct-1")
        .collect();
    assert_eq!(
        person,
        [
            &serde_json::json!({ "subject": "person-acct-1", "namespace": "quality_gold",
            "tables": quality::people_grants::PEOPLE_GOLD, "privileges": ["SELECT"],
            "rows": { "column": "dataset", "in": ["clients"] } })
        ]
    );
    assert!(subjects(&body)
        .iter()
        .any(|s| s == "job_maint_gold quality_gold"));
    let (status, body) = ask_with_people(true).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "iam_unavailable");
}

/// Ruling 84: an open console run's user reads its asker's datasets only.
#[tokio::test]
async fn an_open_console_run_gets_its_askers_datasets() {
    use contracts::ModuleRegistry;
    use infrastructure::module_migrations::PgModuleMigrator;
    use quality::job_grants_api::routes_with_consoles;
    use quality::module::QualityModule;

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
    ] {
        sqlx::query(sql).execute(&db.pool).await.unwrap();
    }
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap();
    for (id, state) in [
        ("0b9c6a1e-0000-4000-8000-000000000091", "queued"),
        ("0b9c6a1e-0000-4000-8000-000000000092", "done"),
    ] {
        sqlx::query(
            "INSERT INTO quality.jobs (id, kind, state, parameters)
             VALUES ($1::uuid, 'query', $2, '{\"sql\": \"SELECT 1\", \"datasets\": [\"clients\"]}')",
        )
        .bind(id)
        .bind(state)
        .execute(&app)
        .await
        .unwrap();
    }
    let router = routes_with_consoles(Arc::new(Main { fails: false }), Arc::new(Tokens), None, app);
    let req = Request::get(URI).header("authorization", "Bearer rules-merge");
    let res = router
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let out: Value = serde_json::from_slice(&bytes).unwrap();
    let console: Vec<&Value> = out["grants"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|g| g["subject"].as_str().unwrap().starts_with("job_console_"))
        .collect();
    assert!(console
        .iter()
        .all(|g| g["subject"] == "job_console_r0b9c6a1e000040008000000000000091"));
    let namespaces: Vec<&str> = console
        .iter()
        .map(|g| g["namespace"].as_str().unwrap())
        .collect();
    assert_eq!(
        namespaces,
        ["clients_bronze", "clients_silver", "quality_gold"]
    );
    assert_eq!(console[2]["rows"]["in"], serde_json::json!(["clients"]));
}
