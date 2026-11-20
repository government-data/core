//! A machine renews its certificate (0066, steps 3 and 4), end to end: the
//! real route, the real use cases, PostgreSQL, a CA and signing requests made
//! in each test, and the client certificate header built as Traefik writes it
//! (URL-encoded, comma-separated base64 DER, the verified one first). Needs
//! Docker.

use std::sync::Arc;
use std::time::Duration;

mod machine_certs;

use application::{Acceptance, LicenseService, MachinesService, PlatformService};
use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::{MachineId, MachineRole};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use infrastructure::ca::{
    certificate_info, Ca, Lifetime, MachineIdentity, X509CertificateReader, DEFAULT_CA_VALIDITY,
};
use infrastructure::{PgAuditRepository, PgMachines, RingJoinTokenMint, SystemClock};
use license_model::{
    Entitlement, FetchError, LicenseAuthority, LicenseStore, Limits, StoredLicense,
};
use machine_certs::{admin, csr, key, origin, traefik_header, Held};
use rcgen::{CertificateParams, DnType, KeyPair};
use serde_json::{json, Value};
use tower::ServiceExt;

const RENEW: &str = "/internal/machines/renew";
const DAY: i64 = 24 * 3600;

struct World {
    app: axum::Router,
    machines: MachinesService,
    /// The same CA the service signs with, to sign what a machine could not
    /// have got from it.
    ca: Ca,
    pool: sqlx::PgPool,
    _db: testkit::TestDb,
}

/// How the world is made: with a CA, trusting the header, read-only.
#[derive(Clone, Copy)]
struct Setup {
    ca: bool,
    trusted: bool,
    read_only: bool,
}

const NORMAL: Setup = Setup {
    ca: true,
    trusted: true,
    read_only: false,
};

async fn world(setup: Setup) -> World {
    let db = testkit::spawn_test_db().await;
    let pool = db.pool.clone();
    let mut machines = MachinesService::new(
        Arc::new(PgMachines::new(pool.clone())),
        Arc::new(RingJoinTokenMint::new()),
        Arc::new(PgAuditRepository::new(pool.clone())),
        Arc::new(SystemClock),
    );
    let made = Ca::create("test", DEFAULT_CA_VALIDITY).unwrap();
    if setup.ca {
        let ca = Ca::load(&made.cert_pem, &made.key_pem).unwrap();
        machines = machines.with_signer(Arc::new(ca));
    }
    let certificates = if setup.trusted {
        ClientCertificates::trusting(Arc::new(X509CertificateReader))
    } else {
        ClientCertificates::not_trusted()
    };
    let mut platform = PlatformService::new(Arc::new(infrastructure::PgPlatformRepository::new(
        pool.clone(),
    )));
    if setup.read_only {
        platform = platform.with_license(suspended_license().await);
        assert!(platform.read_only().is_some());
    }
    let app = api::machines::join::routes(machines.clone(), rate_limit())
        .merge(api::machines::renew::routes(machines.clone(), certificates))
        // As the server wraps every route.
        .layer(axum::middleware::from_fn_with_state(
            platform,
            api::read_only::by_license,
        ));
    World {
        app,
        machines,
        ca: Ca::load(&made.cert_pem, &made.key_pem).unwrap(),
        pool,
        _db: db,
    }
}

fn rate_limit() -> api::rate_limit::RateLimitConfig {
    api::rate_limit::RateLimitConfig {
        max_requests: 100,
        window: Duration::from_secs(60),
    }
}

/// A machine joins, as `/install/join` does.
async fn joined(w: &World, role: MachineRole, name: &str) -> Held {
    machine_certs::joined(&w.machines, role, name).await
}

async fn renew_with(w: &World, header: Option<&str>, csr: &str) -> (StatusCode, Value) {
    let mut request = Request::post(RENEW)
        .header("x-forwarded-for", "203.0.113.5")
        .header("content-type", "application/json");
    if let Some(h) = header {
        request = request.header("x-forwarded-tls-client-cert", h);
    }
    let body = Body::from(json!({ "csr": csr }).to_string());
    let response = w.app.clone().oneshot(request.body(body).unwrap()).await;
    let response = response.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await;
    let body = serde_json::from_slice(&bytes.unwrap()).unwrap_or(Value::Null);
    (status, body)
}

/// `held` renews with a new key; on success, the new one is held.
async fn renew(w: &World, held: &Held) -> (StatusCode, Value, Option<Held>) {
    let key = key();
    let header = traefik_header(&[&held.cert]);
    let (status, body) = renew_with(w, Some(&header), &csr(&key)).await;
    let cert = body["certificate"].as_str().map(str::to_string);
    let next = cert.map(|cert| Held { key, cert });
    (status, body, next)
}

fn serial(held: &Held) -> String {
    certificate_info(&held.cert).unwrap().serial
}

fn now() -> i64 {
    Utc::now().timestamp()
}

/// The audit's machine events, in order: type and action.
async fn audited(w: &World) -> Vec<(String, String)> {
    let rows = sqlx::query_as(
        "SELECT event_type, action FROM admin_audit_events \
         WHERE event_type LIKE 'certificate_%' ORDER BY occurred_at, id",
    );
    rows.fetch_all(&w.pool).await.unwrap()
}

#[tokio::test]
async fn machines_renew_and_the_old_certificate_ends_when_the_new_one_is_used() {
    let w = world(NORMAL).await;
    let a = joined(&w, MachineRole::Worker, "worker-01").await;
    let (status, body, b) = renew(&w, &a).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let b = b.unwrap();
    let info = certificate_info(&b.cert).unwrap();
    assert_eq!(
        (info.organization.as_deref(), info.unit.as_deref()),
        (Some("qd"), Some("worker"))
    );
    assert_eq!(info.common_name.as_deref(), Some("worker-01"));
    assert_eq!(body["serial"].as_str(), Some(info.serial.as_str()));
    assert_eq!(body["not_after"].as_i64(), Some(info.not_after.timestamp()));
    assert!((info.not_after.timestamp() - now() - DAY).abs() < 120);
    assert_eq!(body["state"], json!("pending"));
    assert!(body["ca"].as_str().unwrap().contains("BEGIN CERTIFICATE"));
    assert_ne!(serial(&a), serial(&b));

    // The answer was lost: the machine still holds `a`, accepted, and
    // renews again, which replaces `b`.
    let (status, _, c) = renew(&w, &a).await;
    assert_eq!(status, StatusCode::OK);
    let c = c.unwrap();
    let (status, body, _) = renew(&w, &b).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("certificate_refused")),
        "the next one replaced is refused"
    );
    // `c` is first used: it is the one in use, and `a` is refused.
    let (status, _, d) = renew(&w, &c).await;
    assert_eq!(status, StatusCode::OK);
    let (status, refused, _) = renew(&w, &a).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(refused["message"], body["message"], "one text");
    let accepted = w.machines.accepts("worker-01", &serial(&c)).await.unwrap();
    let Acceptance::Accepted(machine) = accepted else {
        panic!("c is in use");
    };
    let d_serial = serial(&d.unwrap());
    assert_eq!(machine.cert_serial, Some(serial(&c)));
    assert_eq!(machine.cert_serial_next, Some(d_serial));

    let events = audited(&w).await;
    let issued = ("certificate_issued".to_string(), "renew:worker".to_string());
    assert_eq!(events.iter().filter(|e| **e == issued).count(), 3);
    let refused = ("certificate_refused".into(), "renew:certificate".into());
    assert_eq!(events.iter().filter(|e| **e == refused).count(), 2);
    let leaked: Option<String> = sqlx::query_scalar(
        "SELECT action FROM admin_audit_events WHERE action LIKE '%CERTIFICATE%' \
         OR resource LIKE '%CERTIFICATE%' LIMIT 1",
    )
    .fetch_optional(&w.pool)
    .await
    .unwrap();
    assert_eq!(leaked, None, "no certificate in the audit");
}

#[tokio::test]
async fn machines_renew_only_for_the_first_certificate_of_the_header() {
    let w = world(NORMAL).await;
    let a = joined(&w, MachineRole::Worker, "worker-01").await;
    // A certificate the client made itself, as admin, after its real one.
    let mut forged = CertificateParams::new(vec!["root".to_string()]).unwrap();
    forged.distinguished_name.push(DnType::CommonName, "root");
    let unit = DnType::OrganizationalUnitName;
    forged.distinguished_name.push(unit, "admin");
    let forged = forged.self_signed(&key()).unwrap().pem();
    let both = traefik_header(&[&a.cert, &forged]);
    let (status, body) = renew_with(&w, Some(&both), &csr(&key())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let info = certificate_info(body["certificate"].as_str().unwrap()).unwrap();
    assert_eq!(info.unit.as_deref(), Some("worker"));
    assert_eq!(info.common_name.as_deref(), Some("worker-01"));

    // A good certificate second is not believed either.
    let other = joined(&w, MachineRole::Worker, "worker-02").await;
    let stranger = w.ca.sign(
        &csr(&key()),
        &identity("worker", "worker-09"),
        Lifetime::MACHINE,
    );
    let stranger = stranger.unwrap().pem;
    let header = traefik_header(&[&stranger, &other.cert]);
    let (status, _) = renew_with(&w, Some(&header), &csr(&key())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "no such machine");

    for bad in ["garbage", "%ZZ", ""] {
        let (status, body) = renew_with(&w, Some(bad), &csr(&key())).await;
        let code = body["code"].as_str();
        assert_eq!(
            (status, code),
            (StatusCode::UNAUTHORIZED, Some("certificate_refused"))
        );
    }
    let (status, _) = renew_with(&w, None, &csr(&key())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let none = (
        "certificate_refused".to_string(),
        "renew:no-certificate".to_string(),
    );
    let events = audited(&w).await;
    assert_eq!(events.iter().filter(|e| **e == none).count(), 4);
}

fn identity(role: &str, name: &str) -> MachineIdentity {
    MachineIdentity::new(role, name, [name]).unwrap()
}

#[tokio::test]
async fn machines_renew_refuses_the_retired_another_role_and_a_bad_request() {
    let w = world(NORMAL).await;
    let a = joined(&w, MachineRole::Worker, "worker-01").await;
    let bad = renew_with(&w, Some(&traefik_header(&[&a.cert])), "not a request").await;
    assert_eq!(bad.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(bad.1["fields"]["csr"].is_string());
    let p384 = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P384_SHA384).unwrap();
    let p384 = renew_with(&w, Some(&traefik_header(&[&a.cert])), &csr(&p384)).await;
    assert_eq!(p384.0, StatusCode::UNPROCESSABLE_ENTITY);

    // The CA's own certificate for its name and serial, but of another
    // role: refused, as no certificate of the CA says so.
    let lake = w.ca.sign(
        &csr(&a.key),
        &identity("lake", "worker-01"),
        Lifetime::MACHINE,
    );
    let lake = lake.unwrap();
    sqlx::query("UPDATE machines SET cert_serial = $1 WHERE name = 'worker-01'")
        .bind(&lake.serial)
        .execute(&w.pool)
        .await
        .unwrap();
    let (status, _) = renew_with(&w, Some(&traefik_header(&[&lake.pem])), &csr(&key())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "another role");

    let b = joined(&w, MachineRole::Worker, "worker-02").await;
    let id: String = sqlx::query_scalar("SELECT id::text FROM machines WHERE name = 'worker-02'")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    let (id, by, origin) = (MachineId::new(id), admin(), origin());
    w.machines.retire(&id, &by, &origin).await.unwrap();
    let (status, body, _) = renew(&w, &b).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("certificate_refused")),
        "retired"
    );
    let events = audited(&w).await;
    for action in ["renew:invalid-request", "renew:role", "renew:certificate"] {
        let event = ("certificate_refused".to_string(), action.to_string());
        assert!(events.contains(&event), "{action}: {events:?}");
    }
}

#[tokio::test]
async fn machines_renew_is_503_without_a_trusted_header_or_a_ca() {
    let w = world(Setup {
        trusted: false,
        ..NORMAL
    })
    .await;
    let a = joined(&w, MachineRole::Worker, "worker-01").await;
    let (status, body, _) = renew(&w, &a).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Some("client_certificate_not_trusted")
        )
    );
    assert!(audited(&w)
        .await
        .iter()
        .all(|e| e.0 != "certificate_refused"));

    let w = world(Setup {
        ca: false,
        ..NORMAL
    })
    .await;
    let some = w.ca.sign(
        &csr(&key()),
        &identity("worker", "worker-01"),
        Lifetime::MACHINE,
    );
    let header = traefik_header(&[&some.unwrap().pem]);
    let (status, body) = renew_with(&w, Some(&header), &csr(&key())).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("no_ca"))
    );
}

#[tokio::test]
async fn machines_renew_while_the_license_is_read_only() {
    let w = world(Setup {
        read_only: true,
        ..NORMAL
    })
    .await;
    let trino = joined(&w, MachineRole::TrinoCoordinator, "trino-1").await;
    let (status, body, renewed) = renew(&w, &trino).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let info = certificate_info(&renewed.unwrap().cert).unwrap();
    assert!(
        (info.not_after.timestamp() - now() - 365 * DAY).abs() < 120,
        "a year"
    );
    // The guard is on: joining, a write, is refused.
    let request = Request::post("/install/join")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "token": "x", "name": "y", "csr": "z" }).to_string(),
        ));
    let response = w.app.clone().oneshot(request.unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// A license the cloud suspended: the installation is read-only.
async fn suspended_license() -> LicenseService {
    let license = LicenseService::new(
        Arc::new(Cloud),
        Arc::new(Stored::default()),
        "inst-1".into(),
        Some("ORQ-PRUEBA-0001".into()),
        true,
    );
    license.refresh().await.unwrap();
    license
}

struct Cloud;

#[async_trait::async_trait]
impl LicenseAuthority for Cloud {
    async fn fetch(
        &self,
        _: &str,
        _: &str,
        _: Option<&license_model::Usage>,
    ) -> Result<String, FetchError> {
        let entitlement = Entitlement {
            customer: "Synthetic".into(),
            plan: "Plan".into(),
            features: vec![],
            limits: Limits::default(),
            installation: "inst-1".into(),
            issued_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::days(30),
            contract_ends_at: None,
            suspended: Some("synthetic".into()),
        };
        Ok(serde_json::to_string(&entitlement).unwrap())
    }

    fn verify(&self, signed: &str) -> Result<Entitlement, String> {
        serde_json::from_str(signed).map_err(|e| e.to_string())
    }
}

#[derive(Default)]
struct Stored(std::sync::Mutex<StoredLicense>);

#[async_trait::async_trait]
impl LicenseStore for Stored {
    async fn load(&self) -> Result<StoredLicense, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    async fn save_key(&self, key: &str) -> Result<(), String> {
        self.0.lock().unwrap().key = Some(key.into());
        Ok(())
    }
    async fn save_permit(&self, permit: &str) -> Result<(), String> {
        self.0.lock().unwrap().permit = Some(permit.into());
        Ok(())
    }
    async fn record_error(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    async fn clear_permit(&self, _: &str) -> Result<(), String> {
        self.0.lock().unwrap().permit = None;
        Ok(())
    }
    async fn reminders_sent(&self) -> Result<Vec<String>, String> {
        Ok(vec![])
    }
    async fn mark_reminders(&self, _: &[String]) -> Result<(), String> {
        Ok(())
    }
}
