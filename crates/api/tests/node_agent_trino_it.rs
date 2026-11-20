//! The node agent's commands for Trino's coordinator (0066, step 9) against
//! the real routes: `node_agent::trino::ready` and `restart` call
//! `/internal/machines/trino/hold` and `/release` of the in-process router
//! (with the renew route), the use cases and PostgreSQL, a CA made in the
//! test, and the header Traefik would write from the certificate the agent
//! presents. The queue's jobs, Trino's queries, its `/v1/info` and the
//! restart command are fakes. Needs Docker.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod machine_certs;
mod trino_gate;

use application::{MachinesService, TrinoGate as Gate};
use async_trait::async_trait;
use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::MachineRole;
use axum::body::Body;
use axum::http::{header, Request};
use infrastructure::ca::{Ca, X509CertificateReader, DEFAULT_CA_VALIDITY};
use infrastructure::{PgAuditRepository, PgMachines, RingJoinTokenMint, SystemClock};
use machine_certs::{admin, origin, traefik_header};
use node_agent::api::{answer, classify, CallError};
use node_agent::cert::{new_request, Credentials};
use node_agent::ports::{ManualClock, ScriptedCommands};
use node_agent::store::FileStore;
use node_agent::trino::{
    ready, restart, HoldAnswer, Info, Restart, RestartError, TrinoGate, TrinoInfo,
};
use serde_json::{json, Value};
use tower::ServiceExt;
use trino_gate::{Jobs, Queries};

/// The router as the agent's HTTP port.
struct RouterGate(axum::Router);

impl RouterGate {
    async fn post(&self, path: &str, cert: &str, body: Value) -> (u16, Vec<u8>) {
        let request = Request::post(path)
            .header("x-forwarded-for", "203.0.113.8")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-forwarded-tls-client-cert", traefik_header(&[cert]))
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = self.0.clone().oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, bytes.to_vec())
    }
}

#[async_trait]
impl TrinoGate for RouterGate {
    async fn hold(&self, identity: &Credentials, _: &str) -> Result<HoldAnswer, CallError> {
        let path = "/internal/machines/trino/hold";
        let (status, body) = self.post(path, &identity.cert_pem, Value::Null).await;
        if status != 200 {
            return Err(classify(status, None));
        }
        Ok(serde_json::from_slice(&body).unwrap())
    }

    async fn release(&self, identity: &Credentials, _: &str) -> Result<bool, CallError> {
        let path = "/internal/machines/trino/release";
        let (status, body) = self.post(path, &identity.cert_pem, Value::Null).await;
        if status != 200 {
            return Err(classify(status, None));
        }
        let body: Value = serde_json::from_slice(&body).unwrap();
        Ok(body["released"].as_bool().unwrap())
    }
}

/// Trino, started a second ago.
struct Back;

#[async_trait]
impl TrinoInfo for Back {
    async fn info(&self) -> Result<Info, String> {
        Ok(Info {
            starting: false,
            uptime: Duration::from_secs(1),
        })
    }
}

struct World {
    gate: RouterGate,
    machines: MachinesService,
    jobs: Arc<Jobs>,
    queries: Arc<Queries>,
    ca_pem: String,
    dir: PathBuf,
    _db: testkit::TestDb,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn world() -> World {
    let db = testkit::spawn_test_db().await;
    let pool = db.pool.clone();
    let made = Ca::create("test", DEFAULT_CA_VALIDITY).unwrap();
    let (jobs, queries) = (Arc::new(Jobs::default()), Arc::new(Queries::default()));
    let machines = MachinesService::new(
        Arc::new(PgMachines::new(pool.clone())),
        Arc::new(RingJoinTokenMint::new()),
        Arc::new(PgAuditRepository::new(pool.clone())),
        Arc::new(SystemClock),
    )
    .with_signer(Arc::new(Ca::load(&made.cert_pem, &made.key_pem).unwrap()))
    .with_trino(Gate {
        jobs: jobs.clone(),
        queries: queries.clone(),
    });
    let certificates = ClientCertificates::trusting(Arc::new(X509CertificateReader));
    let app = api::machines::renew::routes(machines.clone(), certificates.clone())
        .merge(api::machines::trino::routes(machines.clone(), certificates));
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("trino-agent-it-{}-{nanos}", std::process::id()));
    World {
        gate: RouterGate(app),
        machines,
        jobs,
        queries,
        ca_pem: made.cert_pem,
        dir,
        _db: db,
    }
}

impl World {
    /// The agent's directory for a machine of `role` joined and approved.
    async fn agent(&self, role: MachineRole, name: &str) -> FileStore {
        let held = machine_certs::joined(&self.machines, role, name).await;
        let all = self.machines.list().await.unwrap();
        let m = all.iter().find(|m| m.name == name).unwrap();
        self.machines
            .approve(&m.id, &admin(), &origin())
            .await
            .unwrap();
        let store = FileStore::open(&self.dir.join(name)).unwrap();
        store.write_ca(&self.ca_pem).unwrap();
        let credentials = Credentials {
            key_pem: held.key.serialize_pem(),
            cert_pem: held.cert,
        };
        store.store_next(&credentials).unwrap();
        store.promote().unwrap();
        store
    }

    /// Renews through the real route into `next.pem`, as held mode does.
    async fn renew(&self, store: &FileStore) {
        let request = new_request("trino-1").unwrap();
        let current = store.current().unwrap().unwrap();
        let body = json!({ "csr": request.csr_pem });
        let path = "/internal/machines/renew";
        let (status, bytes) = self.gate.post(path, &current.cert_pem, body).await;
        let issued = answer(status, None, &bytes).unwrap();
        store
            .store_next(&request.credentials(&issued.certificate))
            .unwrap();
    }
}

fn how() -> Restart {
    Restart {
        command: vec!["docker".into(), "restart".into(), "trino-1".into()],
        wait: Duration::from_secs(180),
        poll: Duration::from_secs(2),
    }
}

#[tokio::test]
async fn the_agent_holds_restarts_and_releases_trino_with_its_two_certificates() {
    let w = world().await;
    let store = w.agent(MachineRole::TrinoCoordinator, "trino-1").await;
    w.renew(&store).await;
    let old = store.current().unwrap().unwrap();

    // Busy: a job runs, then Trino runs a query; nothing held either time.
    w.jobs.running.store(1, Ordering::SeqCst);
    assert_eq!(ready(&store, &w.gate).await, Ok(false));
    w.jobs.running.store(0, Ordering::SeqCst);
    w.queries.active.store(1, Ordering::SeqCst);
    assert_eq!(ready(&store, &w.gate).await, Ok(false));
    assert!(!w.jobs.held.load(Ordering::SeqCst));

    // Free: held with the certificate in use.
    w.queries.active.store(0, Ordering::SeqCst);
    assert_eq!(ready(&store, &w.gate).await, Ok(true));
    assert!(w.jobs.held.load(Ordering::SeqCst));

    // The next one taken, Trino restarted, the hold released with it.
    assert!(store.promote().unwrap());
    let commands = ScriptedCommands::default();
    let clock = ManualClock::at(SystemTime::now());
    let done = restart(&store, &w.gate, &Back, &commands, &clock, &how()).await;
    assert_eq!(done, Ok(true));
    assert!(!w.jobs.held.load(Ordering::SeqCst));
    assert_eq!(commands.ran(), [["docker", "restart", "trino-1"]]);

    // Presenting the new one made it the one in use: the old is refused.
    let refused = w.gate.hold(&old, &w.ca_pem).await;
    assert_eq!(refused.unwrap_err(), CallError::Refused);
}

#[tokio::test]
async fn a_worker_cannot_hold_trino_and_a_restart_never_released_keeps_the_hold() {
    let w = world().await;
    let worker = w.agent(MachineRole::Worker, "worker-1").await;
    let refused = ready(&worker, &w.gate).await.unwrap_err();
    assert!(refused.contains("401"), "{refused}");
    assert_eq!(w.jobs.holds.load(Ordering::SeqCst), 0);

    let store = w.agent(MachineRole::TrinoCoordinator, "trino-1").await;
    assert_eq!(ready(&store, &w.gate).await, Ok(true));
    let commands = ScriptedCommands::default();
    commands.answering(&[false]);
    let clock = ManualClock::at(SystemTime::now());
    let failed = restart(&store, &w.gate, &Back, &commands, &clock, &how()).await;
    assert!(matches!(failed, Err(RestartError::Command(_))));
    assert!(w.jobs.held.load(Ordering::SeqCst), "left to expire");
}
