//! A machine asks for its token with its certificate (0066, point 6; RFC
//! 8705 `tls_client_auth`), end to end: the provider's real routes over
//! PostgreSQL, a CA and machines made in the test, and the client
//! certificate header as Traefik writes it. Needs Docker.

mod machine_certs;

use std::sync::Arc;

use application::MachinesService;
use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::{
    AuthorizationServer, ClientAuth, ClientId, ClientSettings, MachineId, MachineRole, OidcClient,
    OidcClientRegistry, ResetTokenGenerator, SessionService,
};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use infrastructure::ca::{Ca, X509CertificateReader, DEFAULT_CA_VALIDITY};
use infrastructure::oidc::{Ed25519Signer, PgAuthorizationCodes, PgOidcClients};
use infrastructure::{
    OAuthSecrets, PgAdminRepository, PgAuditRepository, PgMachines, PgSessionRepository,
    ResetTokenAdapter, RingJoinTokenMint, SecureRandomTokens, SystemClock,
};
use machine_certs::{admin, joined, origin, traefik_header, Held};
use serde_json::Value;
use tower::ServiceExt;

const ISSUER: &str = "https://orq.example.com/api";
const AUDIENCE: &str = "quality-worker";
const SECRET: &str = "a-client-secret-long-enough-for-the-test";

struct World {
    /// The token endpoint, trusting the header.
    app: axum::Router,
    /// The same, with the header not trusted.
    untrusted: axum::Router,
    server: AuthorizationServer,
    machines: MachinesService,
    _db: testkit::TestDb,
}

fn machine_settings() -> ClientSettings {
    ClientSettings {
        client_credentials: true,
        audiences: vec![AUDIENCE.into()],
        ..ClientSettings::default()
    }
}

async fn register(pool: &sqlx::PgPool, id: &str, auth: ClientAuth) {
    let client = OidcClient {
        id: ClientId::parse(id).unwrap(),
        name: id.into(),
        redirect_uris: Vec::new(),
        auth,
        settings: machine_settings(),
    };
    assert!(PgOidcClients::new(pool.clone())
        .insert(&client)
        .await
        .unwrap());
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
    let worker = MachineRole::Worker;
    register(&pool, "qd-worker", ClientAuth::Certificate { role: worker }).await;
    let hash = ResetTokenAdapter.hash(SECRET);
    register(&pool, "qd-api", ClientAuth::Secret { hash }).await;

    let signer = Ed25519Signer::from_pkcs8_base64(&Ed25519Signer::generate().unwrap()).unwrap();
    let server = AuthorizationServer::new(
        Arc::new(PgOidcClients::new(pool.clone())),
        Arc::new(PgAuthorizationCodes::new(pool.clone())),
        Arc::new(ResetTokenAdapter),
        Arc::new(OAuthSecrets),
        Arc::new(signer),
        Arc::new(PgAdminRepository::new(pool.clone())),
        Arc::new(SystemClock),
        ISSUER,
    )
    .with_machines(Arc::new(PgMachines::new(pool.clone())));
    let sessions = SessionService::new(
        Arc::new(PgSessionRepository::new(pool.clone())),
        Arc::new(SecureRandomTokens),
        Arc::new(SystemClock),
        auth_gateway::SessionPolicy::recommended(),
    );
    let routes = |certificates| {
        let (server, sessions) = (server.clone(), sessions.clone());
        auth_gateway::http::provider::routes_with_certificates(server, sessions, certificates)
    };
    World {
        app: routes(ClientCertificates::trusting(Arc::new(
            X509CertificateReader,
        ))),
        untrusted: routes(ClientCertificates::not_trusted()),
        server,
        machines,
        _db: db,
    }
}

/// `POST /oauth/token` for `client_credentials`, with `form` added and the
/// certificate header if `held` is given.
async fn ask(
    app: &axum::Router,
    form: &str,
    held: Option<&Held>,
    basic: Option<&str>,
) -> (StatusCode, Value) {
    let mut request =
        Request::post("/oauth/token").header("content-type", "application/x-www-form-urlencoded");
    if let Some(held) = held {
        let header = traefik_header(&[&held.cert]);
        request = request.header("x-forwarded-tls-client-cert", header);
    }
    if let Some(basic) = basic {
        let value = format!("Basic {}", STANDARD.encode(basic));
        request = request.header("authorization", value);
    }
    let body = format!("grant_type=client_credentials&audience={AUDIENCE}{form}");
    let response = app.clone().oneshot(request.body(Body::from(body)).unwrap());
    let response = response.await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await;
    (status, serde_json::from_slice(&bytes.unwrap()).unwrap())
}

const WORKER: &str = "&client_id=qd-worker";

fn claims(token: &Value) -> Value {
    let jwt = token["access_token"].as_str().unwrap();
    let payload = jwt.split('.').nth(1).unwrap();
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap()
}

async fn id_of(w: &World, name: &str) -> MachineId {
    let all = w.machines.list().await.unwrap();
    all.into_iter().find(|m| m.name == name).unwrap().id
}

/// The one refusal: 401, `invalid_client`, nothing else.
fn refused(answer: (StatusCode, Value)) {
    assert_eq!(answer.0, StatusCode::UNAUTHORIZED, "{}", answer.1);
    assert_eq!(answer.1, serde_json::json!({ "error": "invalid_client" }));
}

#[tokio::test]
async fn oidc_machine_cert_an_approved_worker_gets_15_minutes_until_retired() {
    let w = world().await;
    let a = joined(&w.machines, MachineRole::Worker, "worker-01").await;
    let id = id_of(&w, "worker-01").await;
    refused(ask(&w.app, WORKER, Some(&a), None).await);

    w.machines.approve(&id, &admin(), &origin()).await.unwrap();
    let (status, token) = ask(&w.app, WORKER, Some(&a), None).await;
    assert_eq!(status, StatusCode::OK, "{token}");
    assert_eq!(token["expires_in"], 900);
    let c = claims(&token);
    assert_eq!(c["exp"].as_u64().unwrap() - c["iat"].as_u64().unwrap(), 900);
    assert_eq!(
        (c["client_id"].as_str(), c["machine"].as_str()),
        (Some("qd-worker"), Some("worker-01"))
    );
    assert_eq!(c["sub"], "qd-worker");
    let access = token["access_token"].as_str().unwrap();
    assert_eq!(
        w.server.machine_client(access, AUDIENCE).as_deref(),
        Some("qd-worker")
    );

    // The header not trusted, no certificate, or a secret sent too.
    refused(ask(&w.untrusted, WORKER, Some(&a), None).await);
    refused(ask(&w.app, WORKER, None, None).await);
    let with_secret = format!("{WORKER}&client_secret={SECRET}");
    refused(ask(&w.app, &with_secret, Some(&a), None).await);
    let basic = format!("qd-worker:{SECRET}");
    refused(ask(&w.app, "", Some(&a), Some(&basic)).await);

    w.machines.drain(&id, &admin(), &origin()).await.unwrap();
    assert_eq!(ask(&w.app, WORKER, Some(&a), None).await.0, StatusCode::OK);
    w.machines.retire(&id, &admin(), &origin()).await.unwrap();
    refused(ask(&w.app, WORKER, Some(&a), None).await);
}

#[tokio::test]
async fn oidc_machine_cert_another_role_or_a_replaced_serial_is_refused() {
    let w = world().await;
    let trino = joined(&w.machines, MachineRole::TrinoCoordinator, "trino-01").await;
    let id = id_of(&w, "trino-01").await;
    w.machines.approve(&id, &admin(), &origin()).await.unwrap();
    refused(ask(&w.app, WORKER, Some(&trino), None).await);

    let b = joined(&w.machines, MachineRole::Worker, "worker-02").await;
    let id = id_of(&w, "worker-02").await;
    w.machines.approve(&id, &admin(), &origin()).await.unwrap();
    assert_eq!(ask(&w.app, WORKER, Some(&b), None).await.0, StatusCode::OK);
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    let origin = origin();
    let replaced = w
        .machines
        .record_certificate(&id, "abcdef01", later, &origin);
    replaced.await.unwrap();
    refused(ask(&w.app, WORKER, Some(&b), None).await);
}

#[tokio::test]
async fn oidc_machine_cert_a_secret_client_keeps_its_day_and_ignores_the_header() {
    let w = world().await;
    let a = joined(&w.machines, MachineRole::Worker, "worker-01").await;
    let id = id_of(&w, "worker-01").await;
    w.machines.approve(&id, &admin(), &origin()).await.unwrap();
    let basic = format!("qd-api:{SECRET}");
    for held in [None, Some(&a)] {
        let (status, token) = ask(&w.app, "", held, Some(&basic)).await;
        assert_eq!(status, StatusCode::OK, "{token}");
        assert_eq!(token["expires_in"], 86_400);
        let c = claims(&token);
        assert_eq!(c["client_id"], "qd-api");
        assert!(c.get("machine").is_none());
    }
    // Its certificate is no secret.
    refused(ask(&w.app, "&client_id=qd-api", Some(&a), None).await);

    let discovery = Request::get("/.well-known/openid-configuration").body(Body::empty());
    let response = w.app.clone().oneshot(discovery.unwrap()).await.unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await;
    let doc: Value = serde_json::from_slice(&bytes.unwrap()).unwrap();
    let methods = doc["token_endpoint_auth_methods_supported"]
        .as_array()
        .unwrap();
    assert!(methods.iter().any(|m| m == "tls_client_auth"), "{doc}");
}
