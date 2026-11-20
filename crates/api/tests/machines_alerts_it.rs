//! Grafana's webhook (0072, step 5), end to end on the route: the real use
//! case and rows on PostgreSQL, a CA made in the test, the header as
//! Traefik writes it, bodies as Grafana's (V2's and V3's fields). Needs
//! Docker.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

mod machine_certs;

use application::MachinesService;
use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::{CertificateSigner, CertificateTerm, MachineRole};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::ca::{Ca, X509CertificateReader, DEFAULT_CA_VALIDITY};
use infrastructure::{
    PgAuditRepository, PgMachineAlerts, PgMachines, RingJoinTokenMint, SystemClock,
};
use machine_certs::{admin, csr, key, origin, traefik_header, Held};
use serde_json::{json, Value};
use tower::ServiceExt;

const ALERTS: &str = "/internal/machines/alerts";

struct World {
    app: axum::Router,
    machines: MachinesService,
    unknown: Arc<AtomicU64>,
    db: testkit::TestDb,
    qd_app: Held,
}

async fn world() -> World {
    let db = testkit::spawn_test_db().await;
    let pool = db.pool.clone();
    let made = Ca::create("test", DEFAULT_CA_VALIDITY).unwrap();
    let ca = Ca::load(&made.cert_pem, &made.key_pem).unwrap();
    let machines = MachinesService::new(
        Arc::new(PgMachines::new(pool.clone())),
        Arc::new(RingJoinTokenMint::new()),
        Arc::new(PgAuditRepository::new(pool.clone())),
        Arc::new(SystemClock),
    )
    .with_signer(Arc::new(Ca::load(&made.cert_pem, &made.key_pem).unwrap()))
    .with_machine_alerts(Arc::new(PgMachineAlerts::new(pool.clone())));
    let certificates = ClientCertificates::trusting(Arc::new(X509CertificateReader));
    let unknown = Arc::new(AtomicU64::new(0));
    let counted = unknown.clone();
    let count = api::machines::alerts::UnknownAlerts::new(move |n| {
        counted.fetch_add(n, Ordering::SeqCst);
    });
    // The application VM's machine, as the installer makes it (0066, point
    // 16), and a lake node approved.
    let key = key();
    let term = CertificateTerm::UntilReplaced;
    let signed = CertificateSigner::sign(&ca, &csr(&key), MachineRole::App, "qd-app", term);
    let signed = signed.unwrap();
    sqlx::query(
        "INSERT INTO machines (name, role, state, cert_serial, cert_not_after) \
         VALUES ('qd-app', 'app', 'approved', $1, now() + interval '1 year'), \
                ('lake-a', 'lake', 'approved', NULL, NULL)",
    )
    .bind(&signed.serial)
    .execute(&pool)
    .await
    .unwrap();
    World {
        app: api::machines::alerts::routes(machines.clone(), certificates, count),
        machines,
        unknown,
        db,
        qd_app: Held {
            key,
            cert: signed.pem,
        },
    }
}

/// A Grafana delivery of `alerts`, as V3 saw one.
fn delivery(alerts: Value) -> Value {
    json!({
        "receiver": "qd-mail-and-api", "status": "firing", "state": "alerting",
        "version": "1", "groupKey": "{}:{qd_alert=\"disk_low\", vm=\"lake-a\"}",
        "truncatedAlerts": 0, "orgId": 1, "alerts": alerts,
    })
}

fn disk(status: &str, vm: &str) -> Value {
    json!({
        "status": status,
        "labels": { "alertname": "disk_low", "qd_alert": "disk_low", "severity": "warning",
                    "vm": vm, "mountpoint": "drive1", "grafana_folder": "Machines" },
        "annotations": { "summary_en": "Disk almost full on lake-a (drive1)" },
        "startsAt": "2026-11-17T10:00:00Z", "endsAt": "0001-01-01T00:00:00Z",
        "fingerprint": "0a1b2c3d4e5f6789", "values": { "A": 0.07, "C": 1 },
        "valueString": "[ var='A' value=0.07 ]",
    })
}

async fn post(w: &World, body: &Value, cert: Option<&Held>) -> (StatusCode, Value) {
    let mut request = Request::post(ALERTS)
        .header("x-forwarded-for", "203.0.113.9")
        .header("content-type", "application/json");
    if let Some(held) = cert {
        request = request.header("x-forwarded-tls-client-cert", traefik_header(&[&held.cert]));
    }
    let body = Body::from(body.to_string());
    let response = w.app.clone().oneshot(request.body(body).unwrap()).await;
    let response = response.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await;
    (
        status,
        serde_json::from_slice(&bytes.unwrap()).unwrap_or(Value::Null),
    )
}

/// Every row: (machine's name, alertname, fingerprint, received_at as text).
async fn rows(w: &World) -> Vec<(Option<String>, Option<String>, String, String)> {
    sqlx::query_as(
        "SELECT m.name, a.alertname, a.fingerprint, a.received_at::text \
         FROM machine_alerts a LEFT JOIN machines m ON m.id = a.machine_id \
         ORDER BY a.fingerprint",
    )
    .fetch_all(&w.db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_delivery_twice_changes_nothing_the_second_time_and_resolved_removes_it() {
    let w = world().await;
    let firing = delivery(json!([disk("firing", "lake-a")]));
    let (status, body) = post(&w, &firing, Some(&w.qd_app)).await;
    assert_eq!(
        (status, body),
        (StatusCode::OK, json!({ "applied": 1, "unknown": 0 }))
    );
    let first = rows(&w).await;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].0.as_deref(), Some("lake-a"));

    let (status, _) = post(&w, &firing, Some(&w.qd_app)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rows(&w).await, first, "received_at included");

    let resolved = delivery(json!([disk("resolved", "lake-a")]));
    let (status, _) = post(&w, &resolved, Some(&w.qd_app)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(rows(&w).await.is_empty());
}

#[tokio::test]
async fn an_unknown_vm_changes_nothing_and_is_counted() {
    let w = world().await;
    let body = delivery(json!([disk("firing", "lake-z"), disk("firing", "qd-gone")]));
    let (status, answer) = post(&w, &body, Some(&w.qd_app)).await;
    assert_eq!(
        (status, answer),
        (StatusCode::OK, json!({ "applied": 0, "unknown": 2 }))
    );
    assert!(rows(&w).await.is_empty());
    assert_eq!(w.unknown.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_workers_certificate_gets_403_and_none_401() {
    let w = world().await;
    let worker = machine_certs::joined(&w.machines, MachineRole::Worker, "worker-1").await;
    let all = w.machines.list().await.unwrap();
    let id = &all.iter().find(|m| m.name == "worker-1").unwrap().id;
    w.machines.approve(id, &admin(), &origin()).await.unwrap();
    let body = delivery(json!([disk("firing", "lake-a")]));
    let (status, answer) = post(&w, &body, Some(&worker)).await;
    assert_eq!(
        (status, answer["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("certificate_refused"))
    );
    let (status, _) = post(&w, &body, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(rows(&w).await.is_empty());
}

#[tokio::test]
async fn the_heartbeat_and_a_vmless_datasource_error_keep_rows_with_no_machine() {
    let w = world().await;
    let beat = json!({
        "status": "firing", "fingerprint": "aaaa0000bbbb1111", "startsAt": "2026-11-17T09:00:00Z",
        "labels": { "alertname": "DatasourceError", "qd_alert": "alerting_heartbeat",
                    "rulename": "alerting_heartbeat" },
        "values": null,
    });
    let error = json!({
        "status": "firing", "fingerprint": "ffee000011112222", "startsAt": "2026-11-17T09:00:00Z",
        "labels": { "alertname": "DatasourceError", "rulename": "disk_low",
                    "qd_alert": "disk_low", "severity": "warning", "datasource_uid": "mimir" },
    });
    let (status, _) = post(&w, &delivery(json!([beat, error])), Some(&w.qd_app)).await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<_> = rows(&w).await.into_iter().map(|r| (r.0, r.1)).collect();
    let heartbeat = (None, Some("alerting_heartbeat".to_string()));
    let unavailable = (None, Some("DatasourceError".to_string()));
    assert_eq!(
        names,
        [heartbeat.clone(), unavailable],
        "alive, metrics not available"
    );

    let mut resolved = error;
    resolved["status"] = json!("resolved");
    post(&w, &delivery(json!([resolved])), Some(&w.qd_app)).await;
    let names: Vec<_> = rows(&w).await.into_iter().map(|r| (r.0, r.1)).collect();
    assert_eq!(names, [heartbeat]);
}

#[tokio::test]
async fn a_body_that_is_not_grafanas_gets_422() {
    let w = world().await;
    let mut bad_status = disk("pending", "lake-a");
    let mut bad_time = disk("firing", "lake-a");
    bad_time["startsAt"] = json!("yesterday");
    let mut bad_fingerprint = disk("firing", "lake-a");
    bad_fingerprint["fingerprint"] = json!("../x");
    bad_status["labels"] = json!({});
    for alert in [bad_status, bad_time, bad_fingerprint] {
        let (status, _) = post(&w, &delivery(json!([alert])), Some(&w.qd_app)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    let (status, _) = post(&w, &json!({ "no": "alerts" }), Some(&w.qd_app)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(rows(&w).await.is_empty());
}
