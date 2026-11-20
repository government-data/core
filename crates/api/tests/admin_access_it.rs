//! The administrator's entry through the internal routes (the installer and
//! `orquestador acceso-admin`) with an ephemeral Postgres: with no account, it
//! creates the pending administrator and gives its activation link; with the
//! link they choose their password, and from then on only `force` gives
//! another one (the recovery one). Needs Docker.

use std::sync::Arc;
use std::time::Duration;

use application::{MailSlot, UpdatesService};
use auth_gateway::AuditService;
use auth_gateway::PasswordResetService;
use auth_gateway::{AdminRepository, PasswordPolicy};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain::Email;
use infrastructure::{
    Argon2Hasher, Argon2Params, PgAdminRepository, PgAuditRepository, PgMaintenanceStore,
    PgPasswordResetStore, ResetTokenAdapter, SystemClock,
};
use serde_json::{json, Value};
use tower::ServiceExt;

const INTERNAL: &str = "token-interno-de-prueba-de-al-menos-32";

#[path = "../../server/src/updates_wiring.rs"]
mod updates_wiring;
use updates_wiring::Store;

fn setup(pool: &sqlx::PgPool) -> (axum::Router, PasswordResetService) {
    let service = PasswordResetService::new(
        Arc::new(PgAdminRepository::new(pool.clone())),
        Arc::new(PgPasswordResetStore::new(pool.clone())),
        Arc::new(ResetTokenAdapter),
        Arc::new(Argon2Hasher::new(Argon2Params::owasp_default()).unwrap()),
        Arc::new(SystemClock),
        MailSlot::default(),
        PasswordPolicy::recommended(),
        Duration::from_secs(30 * 60),
    );
    let router = api::internal::router(
        UpdatesService::new(Arc::new(Store(PgMaintenanceStore::new(pool.clone())))),
        Some(api::internal::AdminAccessRoute {
            service: service.clone(),
            audit: AuditService::new(Arc::new(PgAuditRepository::new(pool.clone()))),
            link_base: "https://orq.cliente.co".into(),
        }),
        INTERNAL.into(),
    );
    (router, service)
}

async fn access(app: &axum::Router, token: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/admin-access")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn the_installer_gets_an_activation_link_for_a_pending_first_admin() {
    let db = testkit::spawn_test_db().await;
    let (app, service) = setup(&db.pool);
    let admins = PgAdminRepository::new(db.pool.clone());
    let email = Email::parse("ti@cliente.co").unwrap();

    // Without the token, nothing.
    let (status, _) = access(&app, "otro", json!({ "email": "ti@cliente.co" })).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(admins.count().await.unwrap(), 0);

    let (status, body) = access(&app, INTERNAL, json!({ "email": "TI@cliente.co" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["created"], true);
    assert_eq!(body["emailed"], false);
    let link = body["link"].as_str().unwrap();
    let token = link
        .strip_prefix("https://orq.cliente.co/activate#")
        .unwrap();
    let account = admins.find_by_email(&email).await.unwrap().unwrap();
    assert!(account.password_hash.is_pending_activation());
    assert_eq!(account.role, domain::Role::admin());

    // There is already an administrator: another email creates nothing.
    let (status, _) = access(&app, INTERNAL, json!({ "email": "otro@cliente.co" })).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(admins.count().await.unwrap(), 1);

    // They choose their password with the link; repeating the install no longer
    // touches it.
    service
        .reset(token, "Correct horse battery 9")
        .await
        .unwrap();
    let (status, body) = access(&app, INTERNAL, json!({ "email": "ti@cliente.co" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["created"], false);
    assert_eq!(body["link"], Value::Null);

    // `orquestador acceso-admin`: they lost it, a link to choose another.
    let (status, body) = access(
        &app,
        INTERNAL,
        json!({ "email": "ti@cliente.co", "force": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["link"]
        .as_str()
        .unwrap()
        .starts_with("https://orq.cliente.co/reset-password#"));
}
