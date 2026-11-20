//! Trino's coordinator asks whether it may restart (0066, point 9), end to
//! end on the routes: the real use cases and machines on PostgreSQL, a CA
//! made in the test, the header as Traefik writes it; the queue's jobs and
//! Trino's queries are fakes (the queue's own SQL is tested in `quality`'s
//! `tests/trino_hold.rs`). Needs Docker.

use std::sync::atomic::Ordering;
use std::sync::Arc;

mod machine_certs;
mod trino_gate;

use application::{MachinesService, TrinoGate};
use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::MachineRole;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::ca::{Ca, X509CertificateReader, DEFAULT_CA_VALIDITY};
use infrastructure::{PgAuditRepository, PgMachines, RingJoinTokenMint, SystemClock};
use machine_certs::{admin, origin, traefik_header, Held};
use serde_json::Value;
use tower::ServiceExt;
use trino_gate::{Jobs, Queries};

const HOLD: &str = "/internal/machines/trino/hold";
const RELEASE: &str = "/internal/machines/trino/release";

struct World {
    app: axum::Router,
    machines: MachinesService,
    jobs: Arc<Jobs>,
    queries: Arc<Queries>,
    _db: testkit::TestDb,
}

async fn world(gate: bool, trusted: bool) -> World {
    let db = testkit::spawn_test_db().await;
    let pool = db.pool.clone();
    let made = Ca::create("test", DEFAULT_CA_VALIDITY).unwrap();
    let ca = Ca::load(&made.cert_pem, &made.key_pem).unwrap();
    let (jobs, queries) = (Arc::new(Jobs::default()), Arc::new(Queries::default()));
    let mut machines = MachinesService::new(
        Arc::new(PgMachines::new(pool.clone())),
        Arc::new(RingJoinTokenMint::new()),
        Arc::new(PgAuditRepository::new(pool.clone())),
        Arc::new(SystemClock),
    )
    .with_signer(Arc::new(ca));
    if gate {
        machines = machines.with_trino(TrinoGate {
            jobs: jobs.clone(),
            queries: queries.clone(),
        });
    }
    let certificates = if trusted {
        ClientCertificates::trusting(Arc::new(X509CertificateReader))
    } else {
        ClientCertificates::not_trusted()
    };
    World {
        app: api::machines::trino::routes(machines.clone(), certificates),
        machines,
        jobs,
        queries,
        _db: db,
    }
}

/// A machine of `role` joined, and approved unless `pending`.
async fn machine(w: &World, role: MachineRole, name: &str, pending: bool) -> Held {
    let held = machine_certs::joined(&w.machines, role, name).await;
    if !pending {
        let all = w.machines.list().await.unwrap();
        let m = all.iter().find(|m| m.name == name).unwrap();
        w.machines
            .approve(&m.id, &admin(), &origin())
            .await
            .unwrap();
    }
    held
}

async fn post(w: &World, path: &str, cert: Option<&Held>) -> (StatusCode, Value) {
    let mut request = Request::post(path).header("x-forwarded-for", "203.0.113.9");
    if let Some(held) = cert {
        request = request.header("x-forwarded-tls-client-cert", traefik_header(&[&held.cert]));
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

#[tokio::test]
async fn trino_is_held_only_when_free_and_released_by_its_coordinator() {
    let w = world(true, true).await;
    let trino = machine(&w, MachineRole::TrinoCoordinator, "trino-1", false).await;

    w.jobs.running.store(1, Ordering::SeqCst);
    let (status, body) = post(&w, HOLD, Some(&trino)).await;
    assert_eq!(status, StatusCode::OK);
    let busy = serde_json::json!({ "free": false, "running_jobs": 1, "running_queries": 0 });
    assert_eq!(body, busy);
    assert!(
        !w.jobs.held.load(Ordering::SeqCst),
        "not free: released at once"
    );

    w.jobs.running.store(0, Ordering::SeqCst);
    w.queries.active.store(3, Ordering::SeqCst);
    let (_, body) = post(&w, HOLD, Some(&trino)).await;
    assert_eq!(
        (body["free"].as_bool(), body["running_queries"].as_u64()),
        (Some(false), Some(3))
    );
    assert!(!w.jobs.held.load(Ordering::SeqCst));

    w.queries.down.store(true, Ordering::SeqCst);
    let (_, body) = post(&w, HOLD, Some(&trino)).await;
    assert_eq!(
        (body["free"].as_bool(), body["running_queries"].is_null()),
        (Some(false), true)
    );

    w.queries.down.store(false, Ordering::SeqCst);
    w.queries.active.store(0, Ordering::SeqCst);
    let (status, body) = post(&w, HOLD, Some(&trino)).await;
    assert_eq!(
        (status, body["free"].as_bool()),
        (StatusCode::OK, Some(true))
    );
    assert!(w.jobs.held.load(Ordering::SeqCst));

    let (status, body) = post(&w, RELEASE, Some(&trino)).await;
    assert_eq!(
        (status, body["released"].as_bool()),
        (StatusCode::OK, Some(true))
    );
    assert!(!w.jobs.held.load(Ordering::SeqCst));
    let (_, body) = post(&w, RELEASE, Some(&trino)).await;
    assert_eq!(body["released"].as_bool(), Some(false));
}

#[tokio::test]
async fn only_an_approved_coordinators_certificate_holds_trino() {
    let w = world(true, true).await;
    let worker = machine(&w, MachineRole::Worker, "worker-1", false).await;
    let pending = machine(&w, MachineRole::TrinoCoordinator, "trino-2", true).await;
    for (path, cert) in [
        (HOLD, Some(&worker)),
        (RELEASE, Some(&worker)),
        (HOLD, Some(&pending)),
        (HOLD, None),
    ] {
        let (status, body) = post(&w, path, cert).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(body["code"], "certificate_refused", "{path}");
    }
    assert!(!w.jobs.held.load(Ordering::SeqCst));
    assert_eq!(w.jobs.holds.load(Ordering::SeqCst), 0, "nothing was held");
}

#[tokio::test]
async fn without_trino_or_a_trusted_header_the_routes_answer_503() {
    let no_gate = world(false, true).await;
    let trino = machine(&no_gate, MachineRole::TrinoCoordinator, "trino-1", false).await;
    let (status, body) = post(&no_gate, HOLD, Some(&trino)).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("trino_unavailable"))
    );

    let untrusted = world(true, false).await;
    let trino = machine(&untrusted, MachineRole::TrinoCoordinator, "trino-1", false).await;
    let (status, _) = post(&untrusted, HOLD, Some(&trino)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(untrusted.jobs.holds.load(Ordering::SeqCst), 0);
}
