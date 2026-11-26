//! The job grants route (decision 0090, point 7.2): with the rules merge's
//! machine token, the job users' grants filled from the catalog's `main`
//! (each dataset's source and its rules' references); without one, 401; a
//! failing catalog, 503. Driven without a socket. Synthetic names only.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use quality::catalog::{Catalog, CatalogSnapshot, DatasetMapping, RulesSource};
use quality::job_grants_api::routes;
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
