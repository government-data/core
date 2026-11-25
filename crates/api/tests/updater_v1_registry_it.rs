//! `updater/v1`'s registry token and the modules' roles (quality-sdk
//! decision 0013) end to end on the routes: the real use cases on
//! PostgreSQL, a CA made in the test, the update agent's certificate as
//! the installer signs it and the header as Traefik writes it. Needs
//! Docker.

use std::sync::Arc;

mod machine_certs;

use application::{Installed, MachinesService, UpdatesService};
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

const HOST: &str = "123456789012.dkr.ecr.us-east-1.amazonaws.com";
const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

struct World {
    app: axum::Router,
    machines: MachinesService,
    updates: UpdatesService,
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
    let updates = UpdatesService::new(Arc::new(Store(PgMaintenanceStore::new(pool))));
    let certificates = ClientCertificates::trusting(Arc::new(X509CertificateReader));
    World {
        app: api::updater_v1::routes(updates.clone(), machines.clone(), certificates),
        machines,
        updates,
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

async fn put(w: &World, path: &str, cert: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("PUT")
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

fn token(password: &str, hours: i64) -> Value {
    let expires_at = chrono::Utc::now() + chrono::Duration::hours(hours);
    json!({ "host": HOST, "username": "AWS", "password": password,
            "expires_at": expires_at.to_rfc3339() })
}

#[tokio::test]
async fn the_agent_hands_the_registry_token_and_a_bad_one_keeps_the_one_held() {
    let w = world().await;
    let cert = agent(&w.machines).await;
    let path = "/updater/v1/registry";

    let (status, _) = put(&w, path, &cert, token("first-password", 12)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let held = w.updates.registry_token().expect("held");
    assert_eq!(
        (held.host.as_str(), held.password.as_str()),
        (HOST, "first-password")
    );

    for bad in [
        token("x", 13),
        token("x", -1),
        json!({ "host": "https://registry", "username": "AWS", "password": "x",
                "expires_at": "2099-01-01T00:00:00Z" }),
        json!({ "host": HOST, "username": "AWS", "password": "x", "expires_at": "tomorrow" }),
        json!({ "host": HOST }),
    ] {
        let (status, body) = put(&w, path, &cert, bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
        let why = body["error"].as_str().unwrap_or_default();
        assert!(!why.is_empty() && !why.contains("first-password"), "{why}");
    }
    assert_eq!(
        w.updates.registry_token().unwrap().password,
        "first-password",
        "kept"
    );

    let worker = joined(&w.machines, MachineRole::Worker, "worker-1").await;
    let (status, _) = put(&w, path, &worker.cert, token("stolen", 1)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "not the agent");
    assert_eq!(
        w.updates.registry_token().unwrap().password,
        "first-password"
    );

    let (status, _) = put(&w, path, &cert, token("second-password", 6)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        w.updates.registry_token().unwrap().password,
        "second-password",
        "replaced"
    );
}

#[tokio::test]
async fn the_modules_roles_are_stored_and_a_bad_list_is_refused() {
    let w = world().await;
    let cert = agent(&w.machines).await;
    let image = format!("{HOST}/quality-data/lake-rustfs/rustfs@sha256:{DIGEST}");
    let role = |name: &str| {
        json!({ "name": name, "strategy": "all",
        "services": ["rustfs", "gate", "admin"], "images": [image] })
    };
    let modules = json!({ "modules": [{ "name": "lake-rustfs", "version": "0.2.1",
        "provides": ["lake"], "roles": [role("lake"), role("lake-replica")] }] });
    let (status, _) = put(&w, "/updater/v1/modules", &cert, modules).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let stored = w.updates.installed_modules().await.unwrap();
    let names: Vec<_> = stored[0].roles.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["lake", "lake-replica"]);

    for bad in [
        json!({ "name": "lake", "strategy": "rolling", "services": ["rustfs"], "images": [image] }),
        json!({ "name": "lake", "strategy": "all", "services": [], "images": [image] }),
        json!({ "name": "lake", "strategy": "all", "services": ["rustfs"], "images": ["rustfs:latest"] }),
        role("updater"),
        json!({ "name": "lake", "strategy": "all", "services": ["rustfs"], "images": [image],
                "files": [{ "path": "../compose.yml", "sha256": DIGEST, "content_base64": "" }] }),
    ] {
        let modules = json!({ "modules": [{ "name": "lake-rustfs", "version": "0.2.2",
            "roles": [bad.clone()] }] });
        let (status, _) = put(&w, "/updater/v1/modules", &cert, modules).await;
        assert!(status.is_client_error(), "{bad}: {status}");
    }
    assert_eq!(w.updates.installed_modules().await.unwrap(), stored, "kept");
}
