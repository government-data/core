//! The query engine's lake user (decision 0091, ruling 51) made when the
//! update agent hands the role secrets that carry its keys: the main
//! lake's adapter (the testkit one, behind mTLS) is asked with the lake's
//! admin keys and the keys given; a role without them asks nothing. Needs
//! Docker.

use std::sync::Arc;

mod machine_certs;

use application::{Installed, MachinesService, UpdatesService};
use auth_gateway::http::client_cert::ClientCertificates;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::ca::{Ca, X509CertificateReader, DEFAULT_CA_VALIDITY};
use infrastructure::lake_admin_client::AdminKeySource;
use infrastructure::lake_admins::AdapterLakeAdmins;
use infrastructure::{
    PgAuditRepository, PgMachines, PgMaintenanceStore, RingJoinTokenMint, SystemClock,
};
use machine_certs::{csr, key, origin, traefik_header};
use serde_json::{json, Value};
use testkit::lake::InMemoryLake;
use testkit::lake_admin::{serve, CALLER_ROLE};
use testkit::tls::TestCa;
use tower::ServiceExt;

#[path = "../../server/src/updates_wiring.rs"]
mod updates_wiring;
use updates_wiring::Store;

const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// The main lake's admin keys, kept.
struct Kept;

#[async_trait::async_trait]
impl AdminKeySource for Kept {
    async fn open(
        &self,
        _: &auth_gateway::Lake,
    ) -> Result<Option<auth_gateway::LakeAdminKeys>, domain::RepositoryError> {
        Ok(Some(auth_gateway::LakeAdminKeys {
            access_key: "qd-admin".into(),
            secret_key: "the-admin-secret".into(),
        }))
    }
}

async fn put(app: &axum::Router, path: &str, cert: &str, body: Value) -> StatusCode {
    let request = Request::builder()
        .method("PUT")
        .uri(path)
        .header("x-forwarded-tls-client-cert", traefik_header(&[cert]))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap().status()
}

#[tokio::test]
async fn the_coordinator_s_role_secrets_make_its_lake_user() {
    let db = testkit::spawn_test_db().await;
    let pool = db.pool.clone();
    let tls = TestCa::new();
    let lake = serve(Arc::new(InMemoryLake::default()), &tls).await;
    let identity = tls.machine(CALLER_ROLE, "app-1").bundle();
    let admins = AdapterLakeAdmins::new(Some(&lake.url), &tls.pem, &identity, Arc::new(Kept));
    let made = Ca::create("test", DEFAULT_CA_VALIDITY).unwrap();
    let machines = MachinesService::new(
        Arc::new(PgMachines::new(pool.clone())),
        Arc::new(RingJoinTokenMint::new()),
        Arc::new(PgAuditRepository::new(pool.clone())),
        Arc::new(SystemClock),
    )
    .with_signer(Arc::new(Ca::load(&made.cert_pem, &made.key_pem).unwrap()))
    .with_lake_admin(Arc::new(admins));
    let updates = UpdatesService::new(Arc::new(Store(PgMaintenanceStore::new(pool))));
    let certificates = ClientCertificates::trusting(Arc::new(X509CertificateReader));
    let app = api::updater_v1::routes(updates, machines.clone(), certificates);
    let Installed::Signed { certificate, .. } = machines
        .install_updater("updater", &csr(&key()), None, &origin())
        .await
        .unwrap()
    else {
        panic!("signed");
    };
    let cert = certificate.pem;
    let image = format!("registry.test/engine-trino/trino@sha256:{DIGEST}");
    let role = |name: &str| json!({ "name": name, "strategy": "one", "services": ["trino"], "images": [image] });
    let modules = json!({ "modules": [{ "name": "engine-trino", "version": "0.1.0",
        "roles": [role("trino-coordinator"), role("trino-worker")] }] });
    assert_eq!(
        put(&app, "/updater/v1/modules", &cert, modules).await,
        StatusCode::NO_CONTENT
    );

    let path = "/updater/v1/modules/engine-trino/role-secrets";
    let body = json!({ "roles": { "trino-worker": { "trino-shared-secret": "s3cret" } } });
    assert_eq!(put(&app, path, &cert, body).await, StatusCode::NO_CONTENT);
    assert!(
        lake.lake.snapshot().keys.is_empty(),
        "no lake keys, no user"
    );

    let body = json!({ "roles": { "trino-coordinator": {
        "lake-query-engine-access-key": "trino-coordinator",
        "lake-query-engine-secret-key": "a-secret-the-agent-made-0123456789" } } });
    assert_eq!(put(&app, path, &cert, body).await, StatusCode::NO_CONTENT);
    let keys = lake.lake.snapshot().keys;
    assert_eq!(
        keys.get("trino-coordinator").map(String::as_str),
        Some("a-secret-the-agent-made-0123456789")
    );
    drop(db);
}
