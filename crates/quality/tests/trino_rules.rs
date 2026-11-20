//! The route Trino reads its rules from (decision 0062, point 6), over an
//! ephemeral PostgreSQL with row security per tenant: with the secret in
//! the query, the rules of every tenant's datasets that read a source on
//! Trino; without it, 404; a tenant whose catalog does not answer gives no
//! dataset's rule and the rest is served. Synthetic names only.

pub mod worker;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use quality::catalog::{Catalog, CatalogSnapshot, DatasetMapping};
use quality::machine_certs::MachineCertificates;
use quality::source::Source;
use quality::source_store;
use quality::trino_rules_api::{routes, RulesAccess, RulesKey, Tenants, PATH};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;
use worker::*;

const KEY: &str = "synthetic-key-0123456789-abcdefghij";

/// `dataset`, read from `source`.
fn mapping(dataset: &str, source: Option<&str>) -> DatasetMapping {
    let from = source.map_or(String::new(), |s| {
        format!("from: {{ source: {s}, table: dbo.t }}\n")
    });
    DatasetMapping {
        dataset: dataset.into(),
        yaml: format!(
            "dataset: {dataset}\nsource: {dataset}\n{from}fields:\n  Id: {{ column: id, type: text }}\n"
        ),
    }
}

/// Tenant A: `orders` from `alpha` (on Trino), `clients` from `local` (on
/// DuckDB), `drafted` with no source, `gone` from a source that is not
/// there, and a mapping that does not parse. Tenant B: `loans` from `beta`
/// (on Trino), or no answer when `down`.
struct Mappings {
    down: bool,
}

#[async_trait::async_trait]
impl Catalog for Mappings {
    async fn snapshot(&self, _: Uuid, _: &str) -> Result<Option<CatalogSnapshot>, String> {
        Ok(None)
    }

    async fn mappings(&self, tenant: Uuid) -> Result<Vec<DatasetMapping>, String> {
        if tenant.to_string() == A {
            return Ok(vec![
                mapping("orders", Some("alpha")),
                mapping("clients", Some("local")),
                mapping("drafted", None),
                mapping("gone", Some("nowhere")),
                DatasetMapping {
                    dataset: "broken".into(),
                    yaml: "dataset: [".into(),
                },
            ]);
        }
        if self.down {
            return Err("the catalog does not answer".into());
        }
        Ok(vec![mapping("loans", Some("beta"))])
    }
}

struct Both;

#[async_trait::async_trait]
impl Tenants for Both {
    async fn tenants(&self) -> Result<Vec<tenancy::Tenant>, String> {
        Ok(vec![tenant(A), tenant(B)])
    }
}

async fn source(f: &Fixture, of: &str, name: &str, driver: &str) {
    tenancy::within(tenant(of), async {
        let source = Source {
            name: name.into(),
            driver: driver.into(),
            fields: [("host", "db.example"), ("database", "core")]
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .into(),
            secrets: [("user", "reader"), ("password", "synthetic")]
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .into(),
        };
        source_store::put(&f.app, &Mirror, &source).await.unwrap();
    })
    .await;
}

/// The header this test's port reads as the server's reads the verified
/// certificate: `<role>/<machine>`.
const CERT: &str = "x-test-certificate";

/// Accepts the machines `trino-1` (a coordinator) and `worker-1`, as the
/// header names them; the role asked for must be the header's.
struct Certificates;

#[async_trait::async_trait]
impl MachineCertificates for Certificates {
    async fn machine(&self, headers: &HeaderMap, role: &str) -> Option<String> {
        let value = headers.get(CERT)?.to_str().ok()?;
        let (unit, name) = value.split_once('/')?;
        let known = ["trino-coordinator/trino-1", "worker/worker-1"];
        (unit == role && known.contains(&value)).then(|| name.to_string())
    }
}

/// The route over the fixture's database, each tenant with its sources;
/// with the secret, the certificates, or both.
async fn served_with(down: bool, key: bool, certificates: bool) -> (Fixture, Router) {
    let f = fixture().await;
    source(&f, A, "alpha", "sqlserver").await;
    source(&f, A, "local", "postgres").await;
    // A source of B named as one A's mapping reads: not A's.
    source(&f, B, "nowhere", "sqlserver").await;
    source(&f, B, "beta", "sqlserver").await;
    let machines = Arc::new(Certificates) as Arc<dyn MachineCertificates>;
    let access = RulesAccess {
        key: key.then(|| RulesKey::parse(KEY).unwrap()),
        machines: certificates.then_some(machines),
        client: "qd-api".into(),
        tenants: Arc::new(Both),
    };
    let router = routes(f.app.clone(), Arc::new(Mappings { down }), access);
    (f, router)
}

async fn served(down: bool) -> (Fixture, Router) {
    served_with(down, true, true).await
}

async fn get(router: &Router, uri: &str) -> (StatusCode, Vec<u8>) {
    get_as(router, uri, None).await
}

async fn get_as(router: &Router, uri: &str, cert: Option<&str>) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder().uri(uri);
    if let Some(cert) = cert {
        req = req.header(CERT, cert);
    }
    let req = req.body(Body::empty()).unwrap();
    let res = router.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, body.to_vec())
}

/// The users with a rule for the lake, in order.
fn writers(rules: &Value) -> Vec<(String, String)> {
    rules["tables"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["catalog"] == "lake")
        .map(|r| {
            let text = |k: &str| r[k].as_str().unwrap().to_string();
            (text("user"), text("schema"))
        })
        .collect()
}

#[tokio::test]
async fn the_rules_name_every_tenants_datasets_on_trino_and_no_other() {
    let (_f, router) = served(false).await;
    let (status, body) = get(&router, &format!("{PATH}?key={KEY}")).await;
    assert_eq!(status, StatusCode::OK);
    let rules: Value = serde_json::from_slice(&body).unwrap();
    let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
    assert_eq!(
        writers(&rules),
        [
            pair("_loans", "loans_bronze"),
            pair("_orders", "orders_bronze")
        ]
    );
    let catalogs = rules["catalogs"].as_array().unwrap();
    assert!(catalogs
        .contains(&json!({ "user": "_orders", "catalog": "src_alpha", "allow": "read-only" })));
    assert!(catalogs
        .contains(&json!({ "user": "_loans", "catalog": "src_beta", "allow": "read-only" })));
    let text = rules.to_string();
    for absent in ["_clients", "_drafted", "_gone", "_broken", "src_local"] {
        assert!(!text.contains(absent), "{absent}");
    }
    assert_eq!(catalogs.last().unwrap(), &json!({ "allow": "none" }));
}

#[tokio::test]
async fn without_the_secret_there_is_no_route() {
    let (_f, router) = served(false).await;
    let short = &KEY[..KEY.len() - 1];
    for uri in [
        PATH.to_string(),
        format!("{PATH}?key="),
        format!("{PATH}?key={short}"),
        format!("{PATH}?key={KEY}x"),
        format!("{PATH}?other={KEY}"),
    ] {
        let (status, body) = get(&router, &uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert!(body.is_empty(), "{uri}");
    }
}

#[tokio::test]
async fn trinos_certificate_reads_the_rules_without_a_key() {
    let (_f, router) = served_with(false, false, true).await;
    let (status, body) = get_as(&router, PATH, Some("trino-coordinator/trino-1")).await;
    assert_eq!(status, StatusCode::OK);
    let rules: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(writers(&rules).len(), 2);
    // Another role's machine, an unknown one, none, or a key with no key
    // set: the route is not there.
    for (uri, cert) in [
        (PATH.to_string(), Some("worker/worker-1")),
        (PATH.to_string(), Some("trino-coordinator/trino-2")),
        (PATH.to_string(), None),
        (format!("{PATH}?key={KEY}"), None),
    ] {
        let (status, body) = get_as(&router, &uri, cert).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri} {cert:?}");
        assert!(body.is_empty());
    }
}

#[tokio::test]
async fn with_only_the_key_a_certificate_is_not_asked_for() {
    let (_f, router) = served_with(false, true, false).await;
    let coordinator = Some("trino-coordinator/trino-1");
    let (status, _) = get_as(&router, PATH, coordinator).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get(&router, &format!("{PATH}?key={KEY}")).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_tenant_whose_catalog_does_not_answer_gives_no_rule_and_the_rest_is_served() {
    let (_f, router) = served(true).await;
    let (status, body) = get(&router, &format!("{PATH}?key={KEY}")).await;
    assert_eq!(status, StatusCode::OK);
    let rules: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        writers(&rules),
        [("_orders".to_string(), "orders_bronze".to_string())]
    );
}
