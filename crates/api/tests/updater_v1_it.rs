//! `updater/v1` (decision 0003) end to end on the routes: the real use
//! cases on PostgreSQL, a CA made in the test, the update agent's
//! certificate as the installer signs it and the header as Traefik writes
//! it. Each call answers the agent, and only the agent. What runs comes
//! from a fixed count ([`Running`]): the route is what is tested here, not
//! the store's count. Needs Docker.

use std::sync::Arc;
use std::time::Duration;

mod machine_certs;

use application::{
    DrainStatus, HistoryEntry, Installed, MachinesService, Maintenance, UpdateRequest, UpdaterRow,
    UpdatesService, UpdatesStore,
};
use async_trait::async_trait;
use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::MachineRole;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Utc};
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
    let updates = UpdatesService::new(Arc::new(Running(store)));
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

/// The store on PostgreSQL, but for what runs: two executions running and
/// one schedule due.
struct Running(Store);

#[async_trait]
impl UpdatesStore for Running {
    async fn maintenance(&self) -> Result<Option<Maintenance>, String> {
        self.0.maintenance().await
    }
    async fn begin_maintenance(
        &self,
        why: &str,
        until: DateTime<Utc>,
    ) -> Result<Maintenance, String> {
        self.0.begin_maintenance(why, until).await
    }
    async fn end_maintenance(&self) -> Result<(), String> {
        self.0.end_maintenance().await
    }
    async fn drain_status(&self, horizon: Duration) -> Result<DrainStatus, String> {
        assert_eq!(horizon, Duration::from_secs(60), "the agent's horizon");
        let (running, deploying, queued, scheduled_soon) = (2, 0, 3, 1);
        Ok(DrainStatus {
            running,
            deploying,
            queued,
            scheduled_soon,
        })
    }
    async fn updater(&self) -> Result<UpdaterRow, String> {
        self.0.updater().await
    }
    async fn save_report(&self, report: &Value) -> Result<(), String> {
        self.0.save_report(report).await
    }
    async fn save_policy(&self, policy: &Value, admin_id: &str) -> Result<(), String> {
        self.0.save_policy(policy, admin_id).await
    }
    async fn new_request(&self, admin_id: &str) -> Result<UpdateRequest, String> {
        self.0.new_request(admin_id).await
    }
    async fn mark_notified(&self, a: Option<&str>, r: Option<&str>) -> Result<(), String> {
        self.0.mark_notified(a, r).await
    }
    async fn add_history(&self, entry: &HistoryEntry) -> Result<(), String> {
        self.0.add_history(entry).await
    }
    async fn history(&self, limit: u32) -> Result<Vec<HistoryEntry>, String> {
        self.0.history(limit).await
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
