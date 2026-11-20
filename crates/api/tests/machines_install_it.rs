//! The application VM's machine as the installer makes it (0066, point
//! 16), end to end on PostgreSQL with a CA made in the test: approved, role
//! `app`, its first certificate signed on the spot for a key made as the
//! node agent makes it, and then renewed at the real route as any machine's.
//! Needs Docker.

use std::sync::Arc;

mod machine_certs;

use application::{InstallError, Installed, MachinesService};
use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::{MachineRole, MachineState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::ca::{certificate_info, Ca, X509CertificateReader, DEFAULT_CA_VALIDITY};
use infrastructure::{PgAuditRepository, PgMachines, RingJoinTokenMint, SystemClock};
use machine_certs::{csr, joined, key, origin, traefik_header};
use serde_json::{json, Value};
use tower::ServiceExt;

const YEAR: i64 = 365 * 24 * 3600;

#[tokio::test]
async fn the_installed_app_machine_is_approved_and_renews_as_any_machine() {
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

    let request = node_agent::cert::new_request("qd-app").unwrap();
    let install = |held: Option<String>| {
        let (machines, csr) = (machines.clone(), request.csr_pem.clone());
        async move {
            machines
                .install_app("qd-app", &csr, held.as_deref(), &origin())
                .await
        }
    };
    let Ok(Installed::Signed {
        machine,
        certificate,
        ca,
    }) = install(None).await
    else {
        panic!("signed");
    };
    assert_eq!(
        (machine.role, machine.state),
        (MachineRole::App, MachineState::Approved)
    );
    assert_eq!(ca, made.cert_pem);
    let info = certificate_info(&certificate.pem).unwrap();
    assert_eq!(info.organization.as_deref(), Some("qd"));
    assert_eq!(info.unit.as_deref(), Some("app"));
    assert_eq!(info.common_name.as_deref(), Some("qd-app"));
    assert_eq!(machine.cert_serial.as_deref(), Some(info.serial.as_str()));
    let left = info.not_after.timestamp() - chrono::Utc::now().timestamp();
    assert!((left - YEAR).abs() < 120, "a year: {left}");
    // The agent's credentials read back: the certificate is of its key.
    request.credentials(&certificate.pem).validity().unwrap();

    // Run again holding it: nothing; a worker's name is not taken.
    let again = install(Some(info.serial.to_uppercase())).await.unwrap();
    assert_eq!(again, Installed::Unchanged(machine.clone()));
    joined(&machines, MachineRole::Worker, "worker-01").await;
    let from = origin();
    let taken = machines.install_app("worker-01", &request.csr_pem, None, &from);
    assert_eq!(taken.await, Err(InstallError::NameTaken));
    assert_eq!(machines.list().await.unwrap().len(), 2);

    // Its agent renews with it, as any machine.
    let certificates = ClientCertificates::trusting(Arc::new(X509CertificateReader));
    let app = api::machines::renew::routes(machines.clone(), certificates);
    let body = Body::from(json!({ "csr": csr(&key()) }).to_string());
    let renew = Request::post("/internal/machines/renew")
        .header("x-forwarded-for", "203.0.113.5")
        .header("content-type", "application/json")
        .header(
            "x-forwarded-tls-client-cert",
            traefik_header(&[&certificate.pem]),
        )
        .body(body)
        .unwrap();
    let response = app.oneshot(renew).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await;
    let renewed: Value = serde_json::from_slice(&bytes.unwrap()).unwrap();
    assert_eq!(renewed["state"], json!("approved"));
    let next = certificate_info(renewed["certificate"].as_str().unwrap()).unwrap();
    assert_eq!(next.unit.as_deref(), Some("app"));

    // Events of one second are in no order of their own: sorted here.
    let events: Vec<(String, String)> = sqlx::query_as(
        "SELECT event_type, action FROM admin_audit_events \
         WHERE resource = 'machine/qd-app' ORDER BY event_type DESC, action",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let event = |t: &str, a: &str| (t.to_string(), a.to_string());
    assert_eq!(
        events,
        [
            event("machine_approved", "install:app"),
            event("certificate_issued", "install:app"),
            event("certificate_issued", "renew:app"),
        ]
    );
}
