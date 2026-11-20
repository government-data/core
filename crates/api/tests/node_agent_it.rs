//! The node agent (0066 step 6) against the real routes: its library
//! drives `/install/join` and `/internal/machines/renew` of the in-process
//! router, the use cases and PostgreSQL, with a CA made in the test. The
//! HTTP port calls the router directly and writes the client certificate
//! header from the certificate the agent presents, as Traefik does. Needs
//! Docker.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod machine_certs;

use application::MachinesService;
use async_trait::async_trait;
use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::{Machine, MachineRole, MachineState};
use axum::body::Body;
use axum::http::{header, Request};
use infrastructure::ca::DEFAULT_CA_VALIDITY;
use infrastructure::ca::{certificate_info, sha256_fingerprint, Ca, X509CertificateReader};
use infrastructure::{PgAuditRepository, PgMachines, RingJoinTokenMint, SystemClock};
use machine_certs::{admin, origin, traefik_header};
use node_agent::agent::{Agent, Fatal};
use node_agent::api::{answer, CallError, Issued, MachineApi};
use node_agent::cert::Credentials;
use node_agent::config::{JoinToken, Settings};
use node_agent::plan::{Mode, Step};
use node_agent::ports::{ManualClock, ScriptedCommands};
use node_agent::store::{FileStore, MACHINE_CERT};
use serde_json::json;
use tower::ServiceExt;

const NAME: &str = "worker-07";

/// The router as the agent's HTTP port; `lose` drops the next answer
/// after the router gave it.
#[derive(Clone)]
struct RouterApi {
    app: axum::Router,
    lose: Arc<AtomicBool>,
}

impl RouterApi {
    async fn call(
        &self,
        path: &str,
        cert: Option<&str>,
        body: serde_json::Value,
    ) -> Result<Issued, CallError> {
        let mut request = Request::post(path)
            .header("x-forwarded-for", "203.0.113.7")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(cert) = cert {
            request = request.header("x-forwarded-tls-client-cert", traefik_header(&[cert]));
        }
        let request = request.body(Body::from(body.to_string())).unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        if self.lose.swap(false, Ordering::SeqCst) {
            return Err(CallError::Network("the answer was lost".into()));
        }
        answer(status, None, &bytes)
    }
}

#[async_trait]
impl MachineApi for RouterApi {
    async fn join(&self, token: &str, name: &str, csr: &str) -> Result<Issued, CallError> {
        let body = json!({ "token": token, "name": name, "csr": csr });
        self.call("/install/join", None, body).await
    }

    async fn renew(
        &self,
        identity: &Credentials,
        _ca: &str,
        csr: &str,
    ) -> Result<Issued, CallError> {
        let body = json!({ "csr": csr });
        self.call("/internal/machines/renew", Some(&identity.cert_pem), body)
            .await
    }
}

/// A directory of the test's own.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("node-agent-it-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct World {
    api: RouterApi,
    machines: MachinesService,
    ca_pem: String,
    tmp: TempDir,
    _db: testkit::TestDb,
}

async fn world() -> World {
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
    let certificates = ClientCertificates::trusting(Arc::new(X509CertificateReader));
    let limit = api::rate_limit::RateLimitConfig {
        max_requests: 100,
        window: Duration::from_secs(60),
    };
    let app = api::machines::join::routes(machines.clone(), limit)
        .merge(api::machines::renew::routes(machines.clone(), certificates));
    let api = RouterApi {
        app,
        lose: Arc::default(),
    };
    World {
        api,
        machines,
        ca_pem: made.cert_pem,
        tmp: TempDir::new(),
        _db: db,
    }
}

impl World {
    fn token_file(&self) -> PathBuf {
        self.tmp.0.join("token")
    }

    fn dir(&self) -> PathBuf {
        self.tmp.0.join("agent")
    }

    /// An agent with a fresh join token, renewing every minute (the server
    /// signs on the real clock, so the agent's must not run far ahead).
    async fn agent(&self) -> Agent<RouterApi, ManualClock, ScriptedCommands> {
        let (by, origin) = (admin(), origin());
        let issued = self
            .machines
            .issue_join_token(MachineRole::Worker, None, &by, &origin);
        std::fs::write(self.token_file(), issued.await.unwrap().token).unwrap();
        let settings = Settings {
            dir: self.dir(),
            public_url: Some("https://calidad.example/api".into()),
            internal_url: "https://internal.example:9443/api".into(),
            name: Some(NAME.into()),
            token: Some(JoinToken::File(self.token_file())),
            ca_sha256: Some(sha256_fingerprint(&self.ca_pem).unwrap()),
            ca_file: None,
            renew_every: Duration::from_secs(60),
            after_renew: None,
            mode: Mode::Rolling,
            held_ready: None,
        };
        let store = FileStore::open(&self.dir()).unwrap();
        let clock = ManualClock::at(SystemTime::now());
        let commands = ScriptedCommands::default();
        let mut agent = Agent::new(settings, store, self.api.clone(), clock, commands);
        agent = agent.with_jitter(|| 0.0);
        agent.start().unwrap();
        agent
    }

    async fn machine(&self) -> Machine {
        let all = self.machines.list().await.unwrap();
        all.into_iter().find(|m| m.name == NAME).unwrap()
    }

    fn certificate(&self) -> String {
        std::fs::read_to_string(self.dir().join(MACHINE_CERT)).unwrap()
    }
}

fn serial(cert_pem: &str) -> String {
    certificate_info(cert_pem).unwrap().serial
}

/// Steps until `wanted`, waiting meanwhile.
async fn until<A: MachineApi>(agent: &mut Agent<A, ManualClock, ScriptedCommands>, wanted: Step) {
    for _ in 0..20 {
        let step = agent.step().await.unwrap();
        if step == wanted {
            return;
        }
        assert!(matches!(step, Step::Wait(_)), "{step:?} before {wanted:?}");
    }
    panic!("never {wanted:?}");
}

#[tokio::test]
async fn the_agent_joins_renews_takes_the_new_certificate_and_is_refused_once_retired() {
    let w = world().await;
    let mut agent = w.agent().await;
    assert_eq!(agent.step().await.unwrap(), Step::Join);
    let a = w.certificate();
    let machine = w.machine().await;
    assert_eq!(machine.state, MachineState::Pending);
    assert_eq!(machine.cert_serial, Some(serial(&a)), "the serial recorded");
    assert!(!w.token_file().exists(), "the token's file is gone");
    let info = certificate_info(&a).unwrap();
    assert_eq!(
        (info.unit.as_deref(), info.common_name.as_deref()),
        (Some("worker"), Some(NAME))
    );

    // A renewal: the next serial, taken by the agent at once.
    until(&mut agent, Step::Renew).await;
    let next = agent.store().next().unwrap().unwrap();
    assert_eq!(
        w.machine().await.cert_serial_next,
        Some(serial(&next.cert_pem))
    );
    assert_eq!(agent.step().await.unwrap(), Step::Promote);
    let b = w.certificate();
    assert_eq!(b, next.cert_pem);

    // The answer of the next renewal is lost: `b`, the one presented, is
    // now in use and `a` refused; the agent keeps `b` and renews again.
    w.api.lose.store(true, Ordering::SeqCst);
    until(&mut agent, Step::Renew).await;
    assert_eq!(w.certificate(), b);
    assert!(agent.store().next().unwrap().is_none());
    let machine = w.machine().await;
    assert_eq!(machine.cert_serial, Some(serial(&b)));
    let lost = machine.cert_serial_next.unwrap();
    let old = w
        .api
        .renew(
            &Credentials {
                key_pem: String::new(),
                cert_pem: a,
            },
            "",
            "x",
        )
        .await;
    assert_eq!(old.unwrap_err(), CallError::Refused, "the replaced one");
    until(&mut agent, Step::Renew).await;
    assert_eq!(agent.step().await.unwrap(), Step::Promote);
    let c = w.certificate();
    let machine = w.machine().await;
    assert_eq!(machine.cert_serial_next, Some(serial(&c)));
    assert_ne!(
        Some(lost),
        machine.cert_serial_next,
        "the lost one replaced"
    );

    // Retired: the agent's renewal is refused, and it stops.
    w.machines
        .retire(&machine.id, &admin(), &origin())
        .await
        .unwrap();
    let stopped = loop {
        match agent.step().await {
            Ok(Step::Wait(_)) => continue,
            other => break other,
        }
    };
    assert!(matches!(stopped, Err(Fatal::Refused)), "{stopped:?}");
    assert!(agent.store().refused());
}

#[tokio::test]
async fn a_spent_token_stops_the_agent_and_writes_nothing() {
    let w = world().await;
    let mut first = w.agent().await;
    let token = std::fs::read_to_string(w.token_file()).unwrap();
    assert_eq!(first.step().await.unwrap(), Step::Join);
    // Another machine given the same token, already used.
    let other = TempDir::new();
    std::fs::write(other.0.join("token"), token).unwrap();
    let dir = other.0.join("agent");
    let settings = Settings {
        dir: dir.clone(),
        name: Some("worker-08".into()),
        token: Some(JoinToken::File(other.0.join("token"))),
        ca_sha256: Some(sha256_fingerprint(&w.ca_pem).unwrap()),
        public_url: Some("https://calidad.example/api".into()),
        internal_url: "https://internal.example:9443/api".into(),
        ca_file: None,
        renew_every: Duration::from_secs(60),
        after_renew: None,
        mode: Mode::Rolling,
        held_ready: None,
    };
    let store = FileStore::open(&dir).unwrap();
    let clock = ManualClock::at(SystemTime::now());
    let mut agent = Agent::new(
        settings,
        store,
        w.api.clone(),
        clock,
        ScriptedCommands::default(),
    );
    agent.start().unwrap();
    assert!(matches!(agent.step().await, Err(Fatal::JoinRefused)));
    assert!(agent.store().current().unwrap().is_none());
    assert!(other.0.join("token").exists(), "kept for the person to see");
}
