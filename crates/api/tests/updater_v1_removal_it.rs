//! `GET /updater/v1/removal/{module}` (quality-sdk decision 0015) end to
//! end on the route: the real use cases on PostgreSQL, the update agent's
//! certificate, and two fake modules on a local socket, one refusing and
//! one not answering. Needs Docker.

use std::sync::Arc;
use std::time::Duration;

mod machine_certs;

use application::{Installed, MachinesService, UpdatesService};
use auth_gateway::http::client_cert::ClientCertificates;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::ca::{Ca, X509CertificateReader, DEFAULT_CA_VALIDITY};
use infrastructure::module_removal::HttpModuleRemovals;
use infrastructure::{
    PgAuditRepository, PgMachines, PgMaintenanceStore, RingJoinTokenMint, SystemClock,
};
use machine_certs::{csr, key, origin, traefik_header};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tower::ServiceExt;

#[path = "../../server/src/updates_wiring.rs"]
mod updates_wiring;
use updates_wiring::Store;

struct World {
    app: axum::Router,
    machines: MachinesService,
    _db: testkit::TestDb,
}

async fn world(modules_at: &str) -> World {
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
    let removals = HttpModuleRemovals::plain(modules_at, Duration::from_millis(500));
    let updates = UpdatesService::new(Arc::new(Store(PgMaintenanceStore::new(pool))))
        .with_removal_checks(Arc::new(removals));
    let certificates = ClientCertificates::trusting(Arc::new(X509CertificateReader));
    World {
        app: api::updater_v1::routes(updates.clone(), machines.clone(), certificates),
        machines,
        _db: db,
    }
}

async fn agent(machines: &MachinesService) -> String {
    let installed = machines
        .install_updater("updater", &csr(&key()), None, &origin())
        .await
        .unwrap();
    let Installed::Signed { certificate, .. } = installed else {
        panic!("signed");
    };
    certificate.pem
}

async fn call(w: &World, method: &str, path: &str, cert: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("x-forwarded-tls-client-cert", traefik_header(&[cert]))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = w.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// `orchestrator` refuses the removal of `engine-trino` with counts;
/// `slow` does not answer within the timeout.
async fn fake_modules() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0; 4096];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let line = String::from_utf8_lossy(&buf[..n])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                if line.starts_with("GET /modules/slow/") {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
                let body = if line.contains("module=engine-trino ") {
                    r#"{"refuse": true, "reason": "flows on its line", "counts": {"flows": 2}}"#
                } else {
                    r#"{"refuse": false}"#
                };
                let answer = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(answer.as_bytes()).await;
            });
        }
    });
    base
}

#[tokio::test]
async fn the_agent_gets_every_modules_refusal_and_a_silent_one_refuses() {
    let w = world(&fake_modules().await).await;
    let cert = agent(&w.machines).await;
    let modules = json!({ "modules": [
        { "name": "orchestrator", "version": "1.0.0", "removal_path": "/internal/removal" },
        { "name": "engine-trino", "version": "1.0.0" },
        { "name": "docs", "version": "1.0.0" },
    ] });
    let (status, _) = call(&w, "PUT", "/updater/v1/modules", &cert, modules).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, body) = call(
        &w,
        "GET",
        "/updater/v1/removal/engine-trino",
        &cert,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({ "refusals": [{ "module": "orchestrator", "reason": "flows on its line",
                               "counts": { "flows": 2 } }] })
    );
    let (_, body) = call(&w, "GET", "/updater/v1/removal/docs", &cert, Value::Null).await;
    assert_eq!(body, json!({ "refusals": [] }), "nothing refuses");

    let modules = json!({ "modules": [
        { "name": "orchestrator", "version": "1.0.0", "removal_path": "/internal/removal" },
        { "name": "slow", "version": "1.0.0", "removal_path": "/internal/removal" },
    ] });
    call(&w, "PUT", "/updater/v1/modules", &cert, modules).await;
    let (_, body) = call(&w, "GET", "/updater/v1/removal/docs", &cert, Value::Null).await;
    let refusals = body["refusals"].as_array().unwrap();
    assert_eq!(refusals.len(), 1, "{body}");
    assert_eq!(refusals[0]["module"], "slow");

    let (status, _) = call(&w, "GET", "/updater/v1/removal/Bad", &cert, Value::Null).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
