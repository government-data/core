//! The application VM's agent asks whether Dolt may restart (0066, point
//! 11), end to end on the routes: the real use cases and machines on
//! PostgreSQL, a CA made in the test, the header as Traefik writes it; the
//! catalog's Dolt is a fake (the adapter's gate and Dolt's own list are
//! tested in `infrastructure`'s `tests/catalog_dolt_cert.rs`). Needs Docker.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

mod machine_certs;

use application::{DoltHold, DoltStore, MachinesService};
use async_trait::async_trait;
use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::{CertificateSigner, CertificateTerm, MachineRole};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::ca::{Ca, X509CertificateReader, DEFAULT_CA_VALIDITY};
use infrastructure::{PgAuditRepository, PgMachines, RingJoinTokenMint, SystemClock};
use machine_certs::{admin, csr, key, origin, traefik_header, Held};
use serde_json::{json, Value};
use tower::ServiceExt;

const HOLD: &str = "/internal/machines/dolt/hold";
const RELEASE: &str = "/internal/machines/dolt/release";
const MAIN: &str = "00000000-0000-0000-0000-000000000001";

/// The catalog's Dolt: `busy` operations in flight, and whether held.
#[derive(Default)]
struct Store {
    busy: AtomicU64,
    held: AtomicBool,
    holds: AtomicU64,
}

#[async_trait]
impl DoltStore for Store {
    async fn hold(&self, _ttl: Duration) -> DoltHold {
        self.holds.fetch_add(1, Ordering::SeqCst);
        let busy = self.busy.load(Ordering::SeqCst);
        self.held.store(busy == 0, Ordering::SeqCst);
        DoltHold {
            free: busy == 0,
            in_flight: busy,
            running: (busy == 0).then_some(0),
        }
    }

    fn release(&self) -> bool {
        self.held.swap(false, Ordering::SeqCst)
    }
}

struct World {
    app: axum::Router,
    machines: MachinesService,
    ca: Ca,
    store: Arc<Store>,
    db: testkit::TestDb,
}

async fn world(dolt: bool, trusted: bool) -> World {
    let db = testkit::spawn_test_db().await;
    let pool = db.pool.clone();
    let made = Ca::create("test", DEFAULT_CA_VALIDITY).unwrap();
    let store = Arc::new(Store::default());
    let mut machines = MachinesService::new(
        Arc::new(PgMachines::new(pool.clone())),
        Arc::new(RingJoinTokenMint::new()),
        Arc::new(PgAuditRepository::new(pool.clone())),
        Arc::new(SystemClock),
    )
    .with_signer(Arc::new(Ca::load(&made.cert_pem, &made.key_pem).unwrap()));
    if dolt {
        machines = machines.with_dolt(store.clone());
    }
    let certificates = if trusted {
        ClientCertificates::trusting(Arc::new(X509CertificateReader))
    } else {
        ClientCertificates::not_trusted()
    };
    World {
        app: api::machines::dolt::routes(machines.clone(), certificates),
        machines,
        ca: Ca::load(&made.cert_pem, &made.key_pem).unwrap(),
        store,
        db,
    }
}

/// The application VM's machine in `state`, as the installer makes it
/// (0066, point 16): stored with its first certificate, never joined.
async fn app_machine(w: &World, name: &str, state: &str) -> Held {
    let key = key();
    let term = CertificateTerm::UntilReplaced;
    let signed = CertificateSigner::sign(&w.ca, &csr(&key), MachineRole::App, name, term).unwrap();
    sqlx::query(
        "INSERT INTO machines (tenant_id, name, role, state, cert_serial, cert_not_after) \
         VALUES ($1::uuid, $2, 'app', $3, $4, now() + interval '1 year')",
    )
    .bind(MAIN)
    .bind(name)
    .bind(state)
    .bind(&signed.serial)
    .execute(&w.db.pool)
    .await
    .unwrap();
    Held {
        key,
        cert: signed.pem,
    }
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
async fn dolt_is_held_only_when_free_and_released_by_the_app_machine() {
    let w = world(true, true).await;
    let app = app_machine(&w, "qd-app", "approved").await;

    w.store.busy.store(2, Ordering::SeqCst);
    let (status, body) = post(&w, HOLD, Some(&app)).await;
    assert_eq!(status, StatusCode::OK);
    let busy = json!({ "free": false, "in_flight": 2, "running": null });
    assert_eq!(body, busy);
    assert!(!w.store.held.load(Ordering::SeqCst));

    w.store.busy.store(0, Ordering::SeqCst);
    let (status, body) = post(&w, HOLD, Some(&app)).await;
    let free = json!({ "free": true, "in_flight": 0, "running": 0 });
    assert_eq!((status, body), (StatusCode::OK, free));
    assert!(w.store.held.load(Ordering::SeqCst));

    let (status, body) = post(&w, RELEASE, Some(&app)).await;
    assert_eq!(
        (status, body),
        (StatusCode::OK, json!({ "released": true }))
    );
    let (_, body) = post(&w, RELEASE, Some(&app)).await;
    assert_eq!(body, json!({ "released": false }));
}

#[tokio::test]
async fn only_an_approved_app_machines_certificate_holds_dolt() {
    let w = world(true, true).await;
    let worker = machine_certs::joined(&w.machines, MachineRole::Worker, "worker-1").await;
    let all = w.machines.list().await.unwrap();
    let id = &all.iter().find(|m| m.name == "worker-1").unwrap().id;
    w.machines.approve(id, &admin(), &origin()).await.unwrap();
    let pending = app_machine(&w, "qd-app-2", "pending").await;
    let retired = app_machine(&w, "qd-app-3", "retired").await;
    for (path, cert) in [
        (HOLD, Some(&worker)),
        (RELEASE, Some(&worker)),
        (HOLD, Some(&pending)),
        (HOLD, Some(&retired)),
        (HOLD, None),
    ] {
        let (status, body) = post(&w, path, cert).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(body["code"], "certificate_refused", "{path}");
    }
    assert_eq!(w.store.holds.load(Ordering::SeqCst), 0, "nothing was held");
}

#[tokio::test]
async fn without_a_catalog_or_a_trusted_header_the_routes_answer_503() {
    let no_dolt = world(false, true).await;
    let app = app_machine(&no_dolt, "qd-app", "approved").await;
    let (status, body) = post(&no_dolt, HOLD, Some(&app)).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("dolt_unavailable"))
    );

    let untrusted = world(true, false).await;
    let app = app_machine(&untrusted, "qd-app", "approved").await;
    let (status, _) = post(&untrusted, HOLD, Some(&app)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(untrusted.store.holds.load(Ordering::SeqCst), 0);
}
