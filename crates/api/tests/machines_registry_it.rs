//! `GET /internal/machines/registry` (quality-sdk decision 0013) end to
//! end: the real use cases on PostgreSQL, a CA made in the test, a fake
//! cluster's machines joined as `/install/join` does, a test manifest's
//! roles told as the agent tells them, and the header as Traefik writes it.
//! The token is in no log line and no audit row. Needs Docker.

use std::sync::Arc;

mod machine_certs;

use application::{MachinesService, RegistryToken, UpdatesService};
use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::{MachineRole, MachineState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::ca::{Ca, X509CertificateReader, DEFAULT_CA_VALIDITY};
use infrastructure::{
    PgAuditRepository, PgMachines, PgMaintenanceStore, RingJoinTokenMint, SystemClock,
};
use machine_certs::{admin, joined, origin, traefik_header};
use serde_json::{json, Value};
use tower::ServiceExt;

#[path = "../../server/src/updates_wiring.rs"]
mod updates_wiring;
use updates_wiring::Store;

const PASSWORD: &str = "registry-password-not-to-be-logged";
const PATH: &str = "/internal/machines/registry";

fn image(module: &str, d: char) -> String {
    format!(
        "registry.example.com/quality-data/{module}/svc@sha256:{}",
        d.to_string().repeat(64)
    )
}

async fn get(app: &axum::Router, cert: &str) -> (StatusCode, Option<String>, Value) {
    let request = Request::builder()
        .uri(PATH)
        .header("x-forwarded-tls-client-cert", traefik_header(&[cert]))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let cache = response
        .headers()
        .get("cache-control")
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        cache,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[test]
fn a_machine_of_a_declared_role_gets_the_token_and_its_images_and_none_is_logged() {
    let lines = logging::json_lines("server", || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(scenario());
    });
    let logged = serde_json::to_string(&lines).unwrap();
    assert!(!logged.contains(PASSWORD), "the token was logged");
}

#[allow(clippy::too_many_lines)] // one scenario, in order
async fn scenario() {
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
    let updates = UpdatesService::new(Arc::new(Store(PgMaintenanceStore::new(pool.clone()))));
    let certificates = ClientCertificates::trusting(Arc::new(X509CertificateReader));
    let app = api::machines::registry::routes(machines.clone(), updates.clone(), certificates);

    // Two test modules name the role `worker`; nothing names `trino-worker`.
    let role = |images: Vec<String>| {
        json!({ "name": "worker", "strategy": "one",
        "services": ["runner"], "images": images })
    };
    let modules = serde_json::from_value(json!([
        { "name": "engine-a", "version": "1.0.0",
          "roles": [role(vec![image("engine-a", 'b'), image("engine-a", 'a')])] },
        { "name": "engine-b", "version": "1.0.0",
          "roles": [role(vec![image("engine-a", 'a'), image("engine-b", 'c')])] },
    ]))
    .unwrap();
    updates.set_modules(modules).await.unwrap();

    let worker = joined(&machines, MachineRole::Worker, "worker-1").await;
    let other = joined(&machines, MachineRole::TrinoWorker, "trino-worker-1").await;
    let (status, _, _) = get(&app, &worker.cert).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "pending: not approved yet");
    for m in machines.list().await.unwrap() {
        machines.approve(&m.id, &admin(), &origin()).await.unwrap();
    }

    let (status, _, body) = get(&app, &worker.cert).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "no token yet: {body}"
    );
    updates
        .hold_registry_token(RegistryToken {
            host: "registry.example.com".into(),
            username: "AWS".into(),
            password: PASSWORD.into(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(12),
        })
        .unwrap();

    let (status, cache, body) = get(&app, &worker.cert).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(cache.as_deref(), Some("no-store"));
    assert_eq!(body["host"], "registry.example.com");
    assert_eq!(body["username"], "AWS");
    assert_eq!(body["password"], PASSWORD);
    assert!(body["expires_at"].is_string());
    let union = [
        image("engine-a", 'a'),
        image("engine-a", 'b'),
        image("engine-b", 'c'),
    ];
    assert_eq!(body["images"], json!(union), "sorted, each once");
    let names: Vec<_> = body["modules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].clone())
        .collect();
    assert_eq!(names, [json!("engine-a"), json!("engine-b")], "by name");
    assert_eq!(body["modules"][0]["services"], json!(["runner"]));
    assert_eq!(body["modules"][0]["version"], "1.0.0");

    let (status, _, body) = get(&app, &other.cert).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a role no module declares");
    assert!(body.get("password").is_none());

    let retired = machines
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.name == "worker-1")
        .unwrap();
    machines
        .retire(&retired.id, &admin(), &origin())
        .await
        .unwrap();
    let (status, _, _) = get(&app, &worker.cert).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "retired");
    assert!(machines
        .list()
        .await
        .unwrap()
        .iter()
        .any(|m| m.state == MachineState::Retired));

    let audited: Vec<(String, String)> = sqlx::query_as(
        "SELECT event_type, action FROM admin_audit_events WHERE action LIKE 'registry:%' ORDER BY occurred_at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let actions: Vec<&str> = audited.iter().map(|(_, a)| a.as_str()).collect();
    assert!(actions.contains(&"registry:worker"), "{actions:?}");
    assert!(actions.contains(&"registry:worker:no-token"), "{actions:?}");
    assert!(
        actions.contains(&"registry:trino-worker:no-role"),
        "{actions:?}"
    );
    assert!(actions.contains(&"registry:certificate"), "{actions:?}");
    let rows: Vec<String> =
        sqlx::query_scalar("SELECT row_to_json(a)::text FROM admin_audit_events a")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(
        rows.iter().all(|r| !r.contains(PASSWORD)),
        "the token was audited"
    );
}
