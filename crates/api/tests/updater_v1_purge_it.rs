//! `updater/v1`'s purge of a module and the agent's audit (quality-sdk
//! decision 0009) end to end, on PostgreSQL with the agent's certificate:
//! a purge is refused while the module is installed; once removed, it
//! deletes its clients, its managed policies' assignments and its actions
//! from the catalog, and running it again changes nothing; the agent's
//! events go into the audit chain, sealed in order. Needs Docker.

use std::sync::Arc;

mod machine_certs;

use application::{Installed, InstalledModule, MachinesService, PlatformService, UpdatesService};
use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::{AuthorizationServer, ClientId, ModuleClientsRequest, OidcClientRegistry};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::ca::{Ca, X509CertificateReader, DEFAULT_CA_VALIDITY};
use infrastructure::oidc::{Ed25519Signer, PgAuthorizationCodes, PgOidcClients};
use infrastructure::{
    OAuthSecrets, PgAdminRepository, PgAuditRepository, PgMachines, PgMaintenanceStore,
    PgPlatformRepository, ResetTokenAdapter, RingJoinTokenMint, SystemClock,
};
use machine_certs::{csr, key, origin, traefik_header};
use serde_json::{json, Value};
use tower::ServiceExt;

#[path = "../../server/src/updates_wiring.rs"]
mod updates_wiring;
use updates_wiring::Store;

fn analytics() -> InstalledModule {
    serde_json::from_value(json!({
        "name": "bi-superset", "version": "1.0.0",
        "permissions": {
            "actions": [{ "name": "bi-superset:Author",
                          "description": { "en": "Create charts", "es": "Crear gráficos" } }],
            "policies": [{ "name": "bi-superset:Authors",
                           "description": { "en": "BI authors", "es": "Autores de BI" },
                           "actions": ["bi-superset:Author"] }] }
    }))
    .unwrap()
}

async fn call(
    app: &axum::Router,
    cert: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> StatusCode {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("x-forwarded-tls-client-cert", traefik_header(&[cert]))
        .header("content-type", "application/json");
    let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
    app.clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // one scenario, in order
async fn a_removed_module_is_purged_and_the_agents_events_are_chained() {
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
    let Installed::Signed { certificate, .. } = machines
        .install_updater("updater", &csr(&key()), None, &origin())
        .await
        .unwrap()
    else {
        panic!("signed");
    };
    let cert = certificate.pem;
    let signer = Ed25519Signer::from_pkcs8_base64(&Ed25519Signer::generate().unwrap()).unwrap();
    let clients = Arc::new(PgOidcClients::new(pool.clone()));
    let provider = AuthorizationServer::new(
        clients.clone(),
        Arc::new(PgAuthorizationCodes::new(pool.clone())),
        Arc::new(ResetTokenAdapter),
        Arc::new(OAuthSecrets),
        Arc::new(signer),
        Arc::new(PgAdminRepository::new(pool.clone())),
        Arc::new(SystemClock),
        "https://qd.example.com/api",
    );
    let updates = UpdatesService::new(Arc::new(Store(PgMaintenanceStore::new(pool.clone()))));
    let platform = PlatformService::new(Arc::new(PgPlatformRepository::new(pool.clone())))
        .with_updates(updates.clone());
    let app = api::updater_v1_purge::routes(
        Some(provider.clone()),
        platform.clone(),
        updates.clone(),
        machines,
        ClientCertificates::trusting(Arc::new(X509CertificateReader)),
    );

    // Installed, with a client and an assigned policy.
    updates.set_modules(vec![analytics()]).await.unwrap();
    let body: ModuleClientsRequest = serde_json::from_value(json!({ "machine": {
        "capabilities": ["sql-query"], "audiences": ["trino"],
        "secret": "machine-secret-0123456789abcde" } }))
    .unwrap();
    provider
        .put_module_clients("bi-superset", &body, "https://qd.example.com")
        .await
        .unwrap();
    let ana: String = sqlx::query_scalar(
        "INSERT INTO admin_users (email, password_hash, role) VALUES ('ana@example.com', 'x', 'user')
         RETURNING id::text",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO iam_attachments (principal_kind, principal_id, policy)
         VALUES ('user', $1::uuid, 'bi-superset:Authors')",
    )
    .bind(&ana)
    .execute(&pool)
    .await
    .unwrap();

    let purge = "/updater/v1/modules/bi-superset";
    assert_eq!(
        call(&app, &cert, "DELETE", purge, None).await,
        StatusCode::CONFLICT
    );
    updates.set_modules(vec![]).await.unwrap();
    for _ in 0..2 {
        assert_eq!(
            call(&app, &cert, "DELETE", purge, None).await,
            StatusCode::NO_CONTENT
        );
    }
    let machine_id = ClientId::parse("bi-superset-machine").unwrap();
    assert!(
        clients.find(&machine_id).await.unwrap().is_none(),
        "client deleted"
    );
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM iam_attachments WHERE policy = 'bi-superset:Authors'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(left, 0, "assignments deleted");
    assert!(
        platform.module_actions().await.unwrap().is_empty(),
        "actions gone"
    );

    // The agent's events, in the chain.
    for event in ["ModulePurgeStarted", "ModulePurged"] {
        let body = json!({ "event": event, "module": "bi-superset", "version": "1.0.0",
                           "outcome": "ok" });
        let status = call(&app, &cert, "POST", "/updater/v1/audit", Some(body)).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    let failed = json!({ "event": "ModuleAdded", "module": "bi-superset", "version": "1.0.0",
                         "outcome": "failed", "step": "health", "error": "no answer\nin 60 s" });
    let status = call(&app, &cert, "POST", "/updater/v1/audit", Some(failed)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let bad = json!({ "event": "ModuleStolen", "module": "bi-superset", "version": "1",
                      "outcome": "ok" });
    let status = call(&app, &cert, "POST", "/updater/v1/audit", Some(bad)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let rows: Vec<(String, String, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT event_type, action, chain_hash FROM admin_audit_events \
         WHERE resource = 'module/bi-superset' AND event_type NOT LIKE 'module_client%' \
         ORDER BY chain_seq",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let kinds: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(
        kinds,
        ["module_purge_started", "module_purged", "module_added"]
    );
    assert!(rows.iter().all(|r| r.2.is_some()), "sealed in the chain");
    assert_eq!(
        rows[2].1,
        "version:1.0.0 outcome:failed step:health error:no answer in 60 s"
    );
}
