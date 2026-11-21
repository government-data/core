//! `updater/v1` (decision 0003) end to end on the routes: the real use
//! cases on PostgreSQL, a CA made in the test, the update agent's
//! certificate as the installer signs it and the header as Traefik writes
//! it. Each call answers the agent, and only the agent. What runs comes
//! from fixed modules ([`Running`]): the route is what is tested here, not
//! a module's count. Needs Docker.

use std::sync::Arc;
use std::time::Duration;

mod machine_certs;

use application::{Installed, MachinesService, ModuleWork, ModulesWork, UpdatesService};
use async_trait::async_trait;
use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::MachineRole;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::ca::{Ca, X509CertificateReader, DEFAULT_CA_VALIDITY};
use infrastructure::{
    PgAuditRepository, PgMachines, PgMaintenanceStore, RingJoinTokenMint, SystemClock,
};
use machine_certs::{csr, joined, key, origin, traefik_header};
use serde_json::{json, Value};
use tower::ServiceExt;

#[path = "../../server/src/updates_wiring.rs"]
mod updates_wiring;
use updates_wiring::Store;

struct World {
    app: axum::Router,
    machines: MachinesService,
    _db: testkit::TestDb,
}

async fn world(trusted: bool) -> World {
    let db = testkit::spawn_test_db().await;
    let pool = db.pool.clone();
    let made = Ca::create("test", DEFAULT_CA_VALIDITY).unwrap();
    let machines = MachinesService::new(
        Arc::new(PgMachines::new(pool.clone())),
        Arc::new(RingJoinTokenMint::new()),
        Arc::new(PgAuditRepository::new(pool.clone())),
        Arc::new(SystemClock),
    )
    .with_signer(Arc::new(Ca::load(&made.cert_pem, &made.key_pem).unwrap()));
    let store = Store(PgMaintenanceStore::new(pool));
    let updates = UpdatesService::new(Arc::new(store)).with_modules_work(Arc::new(Running));
    let certificates = if trusted {
        ClientCertificates::trusting(Arc::new(X509CertificateReader))
    } else {
        ClientCertificates::not_trusted()
    };
    World {
        app: api::updater_v1::routes(updates, machines.clone(), certificates),
        machines,
        _db: db,
    }
}

/// What runs: a module with two running, three queued and one schedule
/// due, and a module that does not answer.
struct Running;

#[async_trait]
impl ModulesWork for Running {
    async fn work(&self, horizon: Duration) -> Vec<(String, Result<ModuleWork, String>)> {
        assert_eq!(horizon, Duration::from_secs(60), "the agent's horizon");
        let (running, queued, scheduled_soon) = (2, 3, 1);
        let busy = ModuleWork {
            running,
            queued,
            scheduled_soon,
        };
        vec![
            ("busy".into(), Ok(busy)),
            ("silent".into(), Err("no answer".into())),
        ]
    }
}

/// The update agent's certificate, as `server updater-certificate` gets it.
async fn agent(machines: &MachinesService, name: &str) -> String {
    let installed = machines
        .install_updater(name, &csr(&key()), None, &origin())
        .await
        .unwrap();
    let Installed::Signed { certificate, .. } = installed else {
        panic!("signed");
    };
    certificate.pem
}

async fn call(
    w: &World,
    method: &str,
    path: &str,
    cert: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(cert) = cert {
        request = request.header("x-forwarded-tls-client-cert", traefik_header(&[cert]));
    }
    let body = match body {
        Some(body) => {
            request = request.header("content-type", "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let response = w
        .app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn every_call_answers_the_update_agent() {
    let w = world(true).await;
    let cert = agent(&w.machines, "updater").await;
    let agent = Some(cert.as_str());

    let state = json!({ "phase": "idle", "installed_version": "1.2.0", "modules": {} });
    let (status, _) = call(&w, "PUT", "/updater/v1/state", agent, Some(state)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, orders) = call(&w, "GET", "/updater/v1/orders", agent, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(orders["policy"]["mode"], "approval");
    assert_eq!(orders["policy"]["window"]["start"], "02:00");
    assert_eq!(orders["request"], Value::Null);

    let begin = json!({ "active": true, "reason": "update to 1.3.0", "secs": 600 });
    let (status, body) = call(&w, "PUT", "/updater/v1/maintenance", agent, Some(begin)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["maintenance"]["reason"], "update to 1.3.0");

    let (status, drain) = call(&w, "GET", "/updater/v1/drain?horizon_secs=60", agent, None).await;
    assert_eq!(status, StatusCode::OK);
    let counts = (
        &drain["running"],
        &drain["queued"],
        &drain["scheduled_soon"],
    );
    assert_eq!(counts, (&json!(2), &json!(3), &json!(1)));
    assert_eq!(drain["drained"], false, "two running");
    assert_eq!(drain["deploying"], 0);
    assert_eq!(drain["not_answering"], json!(["silent"]));
    assert_eq!(drain["maintenance"]["reason"], "update to 1.3.0");

    let end = json!({ "active": false });
    let (status, body) = call(&w, "PUT", "/updater/v1/maintenance", agent, Some(end)).await;
    assert_eq!(
        (status, body),
        (StatusCode::OK, json!({ "maintenance": null }))
    );

    let (status, body) = call(&w, "GET", "/updater/v1/version", agent, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "version": domain::VERSION }));
}

#[tokio::test]
async fn a_bad_body_is_refused_with_its_reason() {
    let w = world(true).await;
    let cert = agent(&w.machines, "updater").await;
    let agent = Some(cert.as_str());
    let no_reason = json!({ "active": true, "reason": " ", "secs": 600 });
    let (status, body) = call(&w, "PUT", "/updater/v1/maintenance", agent, Some(no_reason)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().is_some_and(|e| !e.is_empty()));

    let (status, body) = call(&w, "PUT", "/updater/v1/state", agent, Some(json!([1]))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]
        .as_str()
        .is_some_and(|e| e.starts_with("report")));
}

#[tokio::test]
async fn only_the_update_agents_certificate_in_use_is_accepted() {
    let w = world(true).await;
    let old = agent(&w.machines, "updater").await;
    let worker = joined(&w.machines, MachineRole::Worker, "worker-1").await;
    let app = {
        let installed = w
            .machines
            .install_app("qd-app", &csr(&key()), None, &origin())
            .await
            .unwrap();
        let Installed::Signed { certificate, .. } = installed else {
            panic!("signed");
        };
        certificate.pem
    };
    // Signed again by the installer: the one before it is replaced.
    let current = agent(&w.machines, "updater").await;

    let version = "/updater/v1/version";
    for (who, cert) in [
        ("none", None),
        ("a worker", Some(worker.cert.as_str())),
        ("the application VM", Some(app.as_str())),
        ("a replaced certificate", Some(old.as_str())),
    ] {
        let (status, body) = call(&w, "GET", version, cert, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{who}");
        assert!(body["error"].is_string(), "{who}");
        let (status, _) = call(&w, "PUT", "/updater/v1/state", cert, Some(json!({}))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{who}: nothing stored");
    }
    let (status, _) = call(&w, "GET", version, Some(&current), None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn without_a_trusted_header_nothing_answers() {
    let w = world(false).await;
    let cert = agent(&w.machines, "updater").await;
    let (status, body) = call(&w, "GET", "/updater/v1/orders", Some(&cert), None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body["error"].is_string());
}
