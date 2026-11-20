//! The lake's gate (0066, point 10), end to end on the route as a gate's
//! `forwardAuth` calls it through the application's Traefik: the real use cases and machines on
//! PostgreSQL, a CA made in the test, the header as Traefik writes it.
//! Needs Docker.

use std::sync::Arc;

mod machine_certs;

use application::MachinesService;
use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::MachineRole;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::ca::{Ca, X509CertificateReader, DEFAULT_CA_VALIDITY};
use infrastructure::{PgAuditRepository, PgLakePools, PgMachines, RingJoinTokenMint, SystemClock};
use machine_certs::{admin, origin, traefik_header, Held};
use serde_json::Value;
use tower::ServiceExt;

const CHECK: &str = "/internal/machines/lake/check";

struct World {
    app: axum::Router,
    machines: MachinesService,
    db: testkit::TestDb,
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
    .with_signer(Arc::new(Ca::load(&made.cert_pem, &made.key_pem).unwrap()))
    .with_lake_pools(Arc::new(PgLakePools::new(pool.clone())));
    let certificates = if trusted {
        ClientCertificates::trusting(Arc::new(X509CertificateReader))
    } else {
        ClientCertificates::not_trusted()
    };
    World {
        app: api::machines::lake::routes(machines.clone(), certificates),
        machines,
        db,
    }
}

/// A machine of `role` that joined as `name`, approved, and retired when
/// `retire`: its certificate and what a gate's Traefik writes of it.
async fn machine(w: &World, role: MachineRole, name: &str, retire: bool) -> (Held, String) {
    let held = if role == MachineRole::Lake {
        machine_certs::joined_node(&w.machines, name).await
    } else {
        machine_certs::joined(&w.machines, role, name).await
    };
    let all = w.machines.list().await.unwrap();
    let joined = all.iter().find(|m| m.name == name).unwrap();
    w.machines
        .approve(&joined.id, &admin(), &origin())
        .await
        .unwrap();
    if retire {
        w.machines
            .retire(&joined.id, &admin(), &origin())
            .await
            .unwrap();
    }
    // Traefik writes the serial in decimal; the CA's are 16 bytes.
    let serial = u128::from_str_radix(joined.cert_serial.as_deref().unwrap(), 16).unwrap();
    let info = format!(
        "Subject=\"O=qd,OU={},CN={name}\";SerialNumber=\"{serial}\"",
        role.as_str()
    );
    (
        held,
        info.replace('=', "%3D")
            .replace('"', "%22")
            .replace(',', "%2C")
            .replace(';', "%3B"),
    )
}

/// As the gate's `forwardAuth` asks through the application's Traefik: the
/// asker's certificate in the header Traefik writes, the machine at the
/// gate named in the Info header, the original method and path.
async fn check(w: &World, asker: Option<&Held>, named: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::get(CHECK)
        .header("x-forwarded-for", "203.0.113.9")
        .header("x-forwarded-method", "PUT")
        .header("x-forwarded-uri", "/qd-tenant-a/data/part-0.parquet");
    if let Some(held) = asker {
        request = request.header("x-forwarded-tls-client-cert", traefik_header(&[&held.cert]));
    }
    if let Some(named) = named {
        request = request.header("x-forwarded-tls-client-cert-info", named);
    }
    let response = w
        .app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// The refusals audited as `action`.
async fn refusals(w: &World, action: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM admin_audit_events \
         WHERE event_type = 'certificate_refused' AND action = $1",
    )
    .bind(action)
    .fetch_one(&w.db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_gate_asks_for_an_approved_worker_and_gets_an_empty_200() {
    let w = world(true).await;
    let (gate, _) = machine(&w, MachineRole::Lake, "lake-1", false).await;
    let (_, worker) = machine(&w, MachineRole::Worker, "worker-1", false).await;
    for _ in 0..2 {
        let (status, body) = check(&w, Some(&gate), Some(&worker)).await;
        assert_eq!((status, body), (StatusCode::OK, Value::Null));
    }
    assert_eq!(refusals(&w, "lake:certificate").await, 0);
}

#[tokio::test]
async fn a_retired_machine_no_one_named_and_no_gate_are_refused_and_audited() {
    let w = world(true).await;
    let (gate, _) = machine(&w, MachineRole::Lake, "lake-1", false).await;
    let (_, retired) = machine(&w, MachineRole::Worker, "worker-2", true).await;
    let (worker_cert, worker) = machine(&w, MachineRole::Worker, "worker-3", false).await;
    let (status, body) = check(&w, Some(&gate), Some(&retired)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "certificate_refused");
    assert_eq!(refusals(&w, "lake:certificate").await, 1);
    let (status, _) = check(&w, Some(&gate), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(refusals(&w, "lake:unnamed").await, 1);
    // A worker asking by itself, naming itself as a gate would: not a gate.
    let (status, _) = check(&w, Some(&worker_cert), Some(&worker)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(refusals(&w, "lake:gate").await, 1);
    let (status, _) = check(&w, None, Some(&worker)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn without_the_trusted_header_setting_the_gate_answers_503() {
    let w = world(false).await;
    let (gate, _) = machine(&w, MachineRole::Lake, "lake-1", false).await;
    let (_, worker) = machine(&w, MachineRole::Worker, "worker-3", false).await;
    let (status, body) = check(&w, Some(&gate), Some(&worker)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "client_certificate_not_trusted");
}
