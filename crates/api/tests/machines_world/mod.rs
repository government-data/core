//! The machines' routes over PostgreSQL, shared by `machines_it.rs` and
//! `machines_lake_admin_it.rs`: the API with the real use cases, a CA made
//! in each test, a signed-in browser, and a stand-in of a lake's gate on
//! the loopback.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use application::{MachinesService, Origin};
use auth_gateway::{AdminRepository, BootstrapService, LoginService, NewAdmin, PasswordHasher};
use auth_gateway::{AuditService, MachineRole, SessionService};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain::{AdminId, Email, Role};
use infrastructure::ca::{Ca, DEFAULT_CA_VALIDITY};
use infrastructure::lake_http::LakeClient;
use infrastructure::rustfs_admin::RustfsLakeAdmin;
use infrastructure::{
    Argon2Hasher, Argon2Params, EmbeddedInstallFiles, PgAdminRepository, PgAuditRepository,
    PgHealthCheck, PgIpLockoutStore, PgLakeAdminKeys, PgLakePools, PgMachines, PgSessionRepository,
    RingJoinTokenMint, SecretBox, SecureRandomTokens, SystemClock,
};
use rcgen::{CertificateParams, DnType, KeyPair, SignatureAlgorithm};
use serde_json::{json, Value};
use tower::ServiceExt;

pub(crate) const ADMIN: (&str, &str) = ("admin@example.com", "Str0ngEnoughPassphrase!");
pub(crate) const USER: (&str, &str) = ("user@example.com", "AlsoStr0ngEnoughPassphrase!");
pub(crate) const DAY: i64 = 24 * 3600;

pub(crate) struct World {
    pub(crate) app: axum::Router,
    pub(crate) machines: MachinesService,
    pub(crate) pool: sqlx::PgPool,
    /// A stand-in of the lake replica's gate on the loopback, plain HTTP:
    /// the lake's admin API asks a node named `localhost` here.
    pub(crate) gate: std::net::TcpListener,
    pub(crate) _db: testkit::TestDb,
}

pub(crate) fn rate_limit(max_requests: u32) -> api::rate_limit::RateLimitConfig {
    api::rate_limit::RateLimitConfig {
        max_requests,
        window: Duration::from_secs(60),
    }
}

/// The API with the machines' routes, with a CA (and the installation
/// files, at `https://qd.example`) or without one, and a limit of `joins`
/// per address and minute.
pub(crate) async fn world(with_ca: bool, joins: u32) -> World {
    let db = testkit::spawn_test_db().await;
    let pool = db.pool.clone();
    let hasher = Arc::new(Argon2Hasher::new(Argon2Params::owasp_default()).unwrap());
    let admins = Arc::new(PgAdminRepository::new(pool.clone()));
    let policy = auth_gateway::PasswordPolicy::recommended();
    let bootstrap = BootstrapService::new(admins.clone(), hasher.clone(), policy);
    bootstrap
        .create_first_admin(ADMIN.0, ADMIN.1)
        .await
        .unwrap();
    admins
        .insert(&NewAdmin {
            email: Email::parse(USER.0).unwrap(),
            password_hash: hasher.hash(USER.1).await.unwrap(),
            role: Role::parse("user").unwrap(),
        })
        .await
        .unwrap();
    let login = LoginService::new(
        admins.clone(),
        Arc::new(PgIpLockoutStore::new(pool.clone())),
        hasher,
        Arc::new(SystemClock),
        auth_gateway::LockoutPolicy::recommended(),
    );
    let sessions = SessionService::new(
        Arc::new(PgSessionRepository::new(pool.clone())),
        Arc::new(SecureRandomTokens),
        Arc::new(SystemClock),
        auth_gateway::SessionPolicy::recommended(),
    );
    let platform = application::PlatformService::new(Arc::new(
        infrastructure::PgPlatformRepository::new(pool.clone()),
    ));
    let pg = Arc::new(PgMachines::new(pool.clone()));
    let gate = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let lake = LakeClient::from_env(Duration::from_secs(5)).unwrap();
    let keys = Arc::new(PgLakeAdminKeys::new(pool.clone(), sealer()));
    let port = gate.local_addr().unwrap().port();
    let lake_admin = Arc::new(RustfsLakeAdmin::new(lake, keys).at("http", port));
    let mut machines = MachinesService::new(
        pg.clone(),
        Arc::new(RingJoinTokenMint::new()),
        Arc::new(PgAuditRepository::new(pool.clone())),
        Arc::new(SystemClock),
    )
    .with_lake_admin_keys(Arc::new(PgLakeAdminKeys::new(pool.clone(), sealer())))
    .with_lake_pools(Arc::new(PgLakePools::new(pool.clone())))
    .with_lake_admin(lake_admin.clone())
    .with_main_lake(Some(format!("http://localhost:{port}")))
    .with_replication(
        lake_admin.clone(),
        lake_admin,
        Arc::new(infrastructure::PgReplicationState::new(pool.clone())),
        Arc::new(OneWarehouse),
    );
    if with_ca {
        let made = Ca::create("test", DEFAULT_CA_VALIDITY).unwrap();
        let ca = Ca::load(&made.cert_pem, &made.key_pem).unwrap();
        let files = Arc::new(EmbeddedInstallFiles::new(Some(&made.cert_pem)).unwrap());
        machines = machines
            .with_signer(Arc::new(ca))
            .with_install(files, pg, "https://qd.example");
    }
    let app = api::router(
        application::HealthService::new(Arc::new(PgHealthCheck::new(pool.clone()))),
        login,
        sessions.clone(),
        AuditService::new(Arc::new(PgAuditRepository::new(pool.clone()))),
        admins,
        None,
        None,
        None,
        platform.clone(),
        None,
        None,
        &[],
        rate_limit(100),
    )
    .merge(api::machines::routes(machines.clone(), sessions))
    .merge(api::machines::join::routes(
        machines.clone(),
        rate_limit(joins),
    ))
    .layer(axum::Extension(api::iam::resolver(platform)));
    World {
        app,
        machines,
        pool,
        gate,
        _db: db,
    }
}

/// One tenant's warehouse, for the replication.
pub(crate) struct OneWarehouse;

pub(crate) const WAREHOUSE: &str = "qd-tenant-00000000000000000000000000000001";

#[async_trait::async_trait]
impl auth_gateway::Warehouses for OneWarehouse {
    async fn warehouses(&self) -> Result<Vec<String>, domain::RepositoryError> {
        Ok(vec![WAREHOUSE.into()])
    }
}

/// The settings' key the lakes' admin keys are sealed with here.
pub(crate) fn sealer() -> SecretBox {
    SecretBox::from_key(&[7; 32]).unwrap()
}

/// A signed-in browser: its session cookie and CSRF token.
pub(crate) struct Client {
    pub(crate) session: String,
    pub(crate) csrf: String,
}

pub(crate) fn cookie(response: &axum::http::Response<Body>, name: &str) -> String {
    let prefix = format!("{name}=");
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .find_map(|v| {
            let pair = v.to_str().ok()?.split(';').next()?;
            pair.strip_prefix(&prefix).map(str::to_string)
        })
        .unwrap()
}

pub(crate) async fn sign_in(app: &axum::Router, (email, password): (&str, &str)) -> Client {
    let body = json!({ "email": email, "password": password }).to_string();
    let request = Request::post("/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    Client {
        session: cookie(&response, "session"),
        csrf: cookie(&response, "csrf"),
    }
}

/// A response's status and JSON body (`Null` when empty). With a client,
/// its cookies; `csrf` says whether the header is sent too.
pub(crate) async fn call(
    app: &axum::Router,
    (method, uri): (&str, &str),
    auth: Option<(&Client, bool)>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-forwarded-for", "203.0.113.5")
        .header("content-type", "application/json");
    if let Some((c, csrf)) = auth {
        let cookies = format!("session={}; csrf={}", c.session, c.csrf);
        request = request.header("cookie", cookies);
        if csrf {
            request = request.header("x-csrf-token", &c.csrf);
        }
    }
    let body = body.map_or(Body::empty(), |b| Body::from(b.to_string()));
    let response = app.clone().oneshot(request.body(body).unwrap()).await;
    let response = response.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20);
    let bytes = bytes.await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A signing request of a new key, asking for a subject and names the CA
/// must not give.
pub(crate) fn csr(alg: &'static SignatureAlgorithm) -> String {
    let key = KeyPair::generate_for(alg).unwrap();
    let mut params = CertificateParams::new(vec!["evil.example".to_string()]).unwrap();
    params.distinguished_name.push(DnType::CommonName, "root");
    let unit = DnType::OrganizationalUnitName;
    params.distinguished_name.push(unit, "admin");
    params.serialize_request(&key).unwrap().pem().unwrap()
}

pub(crate) fn good_csr() -> String {
    csr(&rcgen::PKCS_ECDSA_P256_SHA256)
}

pub(crate) fn origin() -> Origin {
    Origin {
        ip: "192.0.2.1".into(),
        user_agent: None,
    }
}

pub(crate) async fn new_token(w: &World, role: MachineRole) -> String {
    let by = AdminId::new("00000000-0000-0000-0000-0000000000ad");
    let origin = origin();
    let issued = w.machines.issue_join_token(role, None, &by, &origin);
    issued.await.unwrap().token
}

pub(crate) async fn join(w: &World, token: &str, name: &str, csr: &str) -> (StatusCode, Value) {
    let body = json!({ "token": token, "name": name, "csr": csr });
    call(&w.app, ("POST", "/install/join"), None, Some(body)).await
}

/// How far `at` (epoch seconds) is from now plus `seconds`.
pub(crate) fn off(at: i64, seconds: i64) -> i64 {
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH);
    (at - (now.unwrap().as_secs() as i64 + seconds)).abs()
}

/// Answers one request on the stand-in gate with `body` (200), and gives
/// back the request's head.
pub(crate) fn answer_once(
    gate: &std::net::TcpListener,
    body: &'static str,
) -> std::thread::JoinHandle<String> {
    use std::io::{Read, Write};
    let gate = gate.try_clone().unwrap();
    std::thread::spawn(move || {
        let (mut conn, _) = gate.accept().unwrap();
        let (mut head, mut buf) = (Vec::new(), [0u8; 1024]);
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = conn.read(&mut buf).unwrap();
            head.extend_from_slice(&buf[..n]);
        }
        let reply = format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        conn.write_all(reply.as_bytes()).unwrap();
        String::from_utf8(head).unwrap()
    })
}

/// The stand-in gate answers its next requests, one per connection, with
/// each of `answers` (status line, body) in turn, and gives back each
/// request's head.
pub(crate) fn answer_each(
    gate: &std::net::TcpListener,
    answers: Vec<(&'static str, &'static str)>,
) -> std::thread::JoinHandle<Vec<String>> {
    use std::io::{Read, Write};
    let gate = gate.try_clone().unwrap();
    std::thread::spawn(move || {
        let mut heads = Vec::new();
        for (status, body) in answers {
            let (mut conn, _) = gate.accept().unwrap();
            let (mut head, mut buf) = (Vec::new(), [0u8; 1024]);
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = conn.read(&mut buf).unwrap();
                head.extend_from_slice(&buf[..n]);
            }
            let reply = format!(
                "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            conn.write_all(reply.as_bytes()).unwrap();
            heads.push(String::from_utf8(head).unwrap());
        }
        heads
    })
}
