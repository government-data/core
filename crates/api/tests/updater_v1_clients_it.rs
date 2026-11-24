//! `updater/v1`'s calls for a module's OIDC clients (quality-sdk decision
//! 0009) end to end: the real provider and use cases on PostgreSQL, the
//! update agent's certificate as Traefik passes it. The agent registers its
//! clients (twice is once), gets a machine token with its secret, disables
//! them (no token), and cannot take over a client registered by hand; each
//! change is in the audit chain, and no secret or hash in any log line.
//! Needs Docker.

use std::sync::Arc;

mod machine_certs;

use application::{Installed, MachinesService, UpdatesService};
use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::{
    AuthorizationServer, ClientAuth, ClientId, OidcClient, OidcClientRegistry, TokenRequest,
};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::ca::{Ca, X509CertificateReader, DEFAULT_CA_VALIDITY};
use infrastructure::oidc::{Ed25519Signer, PgAuthorizationCodes, PgOidcClients};
use infrastructure::{
    OAuthSecrets, PgAdminRepository, PgAuditRepository, PgMachines, PgMaintenanceStore,
    ResetTokenAdapter, RingJoinTokenMint, SystemClock,
};
use machine_certs::{csr, key, origin, traefik_header};
use serde_json::{json, Value};
use tower::ServiceExt;

#[path = "../../server/src/updates_wiring.rs"]
mod updates_wiring;
use updates_wiring::Store;

const PEOPLE: &str = "people-secret-0123456789abcdef";
const MACHINE: &str = "machine-secret-0123456789abcde";

struct World {
    app: axum::Router,
    cert: String,
    provider: AuthorizationServer,
    pool: sqlx::PgPool,
}

async fn world(db: &testkit::TestDb) -> World {
    let pool = db.pool.clone();
    let made = Ca::create("test", DEFAULT_CA_VALIDITY).unwrap();
    let machines = MachinesService::new(
        Arc::new(PgMachines::new(pool.clone())),
        Arc::new(RingJoinTokenMint::new()),
        Arc::new(PgAuditRepository::new(pool.clone())),
        Arc::new(SystemClock),
    )
    .with_signer(Arc::new(Ca::load(&made.cert_pem, &made.key_pem).unwrap()));
    let installed = machines
        .install_updater("updater", &csr(&key()), None, &origin())
        .await
        .unwrap();
    let Installed::Signed { certificate, .. } = installed else {
        panic!("signed");
    };
    let signer = Ed25519Signer::from_pkcs8_base64(&Ed25519Signer::generate().unwrap()).unwrap();
    let provider = AuthorizationServer::new(
        Arc::new(PgOidcClients::new(pool.clone())),
        Arc::new(PgAuthorizationCodes::new(pool.clone())),
        Arc::new(ResetTokenAdapter),
        Arc::new(OAuthSecrets),
        Arc::new(signer),
        Arc::new(PgAdminRepository::new(pool.clone())),
        Arc::new(SystemClock),
        "https://qd.example.com/api",
    );
    let updates = UpdatesService::new(Arc::new(Store(PgMaintenanceStore::new(pool.clone()))));
    let app = api::updater_v1_clients::routes(
        Some(provider.clone()),
        "https://qd.example.com".into(),
        updates,
        machines,
        ClientCertificates::trusting(Arc::new(X509CertificateReader)),
    );
    World {
        app,
        cert: certificate.pem,
        provider,
        pool,
    }
}

async fn call(w: &World, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("x-forwarded-tls-client-cert", traefik_header(&[&w.cert]))
        .header("content-type", "application/json");
    let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
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

fn clients_body() -> Value {
    json!({
        "people": { "redirect_paths": ["/bi/oauth-authorized/qd"], "secret": PEOPLE,
                    "role_claim": { "name": "bi_roles",
                                    "values": { "author": "bi-superset:Author" } } },
        "machine": { "capabilities": ["sql-query"], "audiences": ["trino"], "secret": MACHINE },
    })
}

fn machine_token() -> TokenRequest {
    TokenRequest {
        grant_type: Some("client_credentials".into()),
        client_id: Some("bi-superset-machine".into()),
        client_secret: Some(MACHINE.into()),
        ..TokenRequest::default()
    }
}

#[test]
fn the_agent_registers_and_disables_a_modules_clients_and_no_secret_is_logged() {
    let lines = logging::json_lines("server", || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(scenario());
    });
    let logged = serde_json::to_string(&lines).unwrap();
    for secret in [PEOPLE, MACHINE] {
        assert!(!logged.contains(secret), "a secret was logged");
    }
    assert!(!logged.contains("secret_hash"), "a hash was logged");
}

async fn scenario() {
    let db = testkit::spawn_test_db().await;
    let w = world(&db).await;
    let path = "/updater/v1/modules/bi-superset/clients";
    for _ in 0..2 {
        let (status, body) = call(&w, "PUT", path, Some(clients_body())).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body,
            json!({ "issuer": "https://qd.example.com/api", "people_client_id": "bi-superset",
                    "machine_client_id": "bi-superset-machine" })
        );
    }
    let people = PgOidcClients::new(w.pool.clone())
        .find(&ClientId::parse("bi-superset").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        people.redirect_uris,
        ["https://qd.example.com/bi/oauth-authorized/qd"]
    );
    let token = w.provider.token(&machine_token()).await.unwrap();
    assert!(!token.access_token.is_empty());

    let (status, _) = call(&w, "POST", &format!("{path}/disable"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        w.provider.token(&machine_token()).await.is_err(),
        "disabled"
    );

    // A client registered by hand is not taken over.
    let hand = OidcClient {
        id: ClientId::parse("grafana").unwrap(),
        name: "Grafana".into(),
        redirect_uris: vec!["https://qd.example.com/grafana/cb".into()],
        auth: ClientAuth::Secret { hash: "x".into() },
        settings: auth_gateway::ClientSettings::default(),
    };
    PgOidcClients::new(w.pool.clone())
        .upsert(&hand)
        .await
        .unwrap();
    let grafana = "/updater/v1/modules/grafana/clients";
    let (status, body) = call(&w, "PUT", grafana, Some(clients_body())).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    let events: Vec<(String, String)> = sqlx::query_as(
        "SELECT event_type, action FROM admin_audit_events \
         WHERE resource = 'module/bi-superset' ORDER BY event_type DESC, action",
    )
    .fetch_all(&w.pool)
    .await
    .unwrap();
    let kinds: Vec<&str> = events.iter().map(|(e, _)| e.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "module_client_registered",
            "module_client_registered",
            "module_client_disabled",
            "module_client_disabled",
        ]
    );
    assert_eq!(events[0].1, "client:bi-superset version: outcome:ok");
}
