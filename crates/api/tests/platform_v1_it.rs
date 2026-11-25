//! `/v1/…` (quality-sdk decisions 0002 and 0015) end to end: a module's
//! machine client, registered as the update agent registers it, gets a
//! token for the core and reads the installed modules; a token for another
//! audience, a person's or none is refused. Needs Docker.

use std::sync::Arc;

use application::UpdatesService;
use auth_gateway::{AuthorizationServer, ModuleClientsRequest, TokenRequest};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::oidc::{Ed25519Signer, PgAuthorizationCodes, PgOidcClients};
use infrastructure::{
    OAuthSecrets, PgAdminRepository, PgMaintenanceStore, ResetTokenAdapter, SystemClock,
};
use serde_json::{json, Value};
use tower::ServiceExt;

#[path = "../../server/src/updates_wiring.rs"]
mod updates_wiring;
use updates_wiring::Store;

const SECRET: &str = "machine-secret-0123456789abcde";

async fn get(app: &axum::Router, token: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::builder().uri("/v1/modules");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
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

async fn token(provider: &AuthorizationServer, audience: &str) -> String {
    let request = TokenRequest {
        grant_type: Some("client_credentials".into()),
        client_id: Some("orchestrator-machine".into()),
        client_secret: Some(SECRET.into()),
        audience: Some(audience.into()),
        ..TokenRequest::default()
    };
    provider.token(&request).await.unwrap().access_token
}

#[tokio::test]
async fn a_modules_machine_client_reads_the_installed_modules() {
    let db = testkit::spawn_test_db().await;
    let pool = db.pool.clone();
    let signer = Ed25519Signer::from_pkcs8_base64(&Ed25519Signer::generate().unwrap()).unwrap();
    let provider = AuthorizationServer::new(
        Arc::new(PgOidcClients::new(pool.clone())),
        Arc::new(PgAuthorizationCodes::new(pool.clone())),
        Arc::new(ResetTokenAdapter),
        Arc::new(OAuthSecrets),
        Arc::new(signer),
        Arc::new(PgAdminRepository::new(pool.clone())),
        Arc::new(SystemClock),
        "https://qd.example.com/api",
    );
    let updates = UpdatesService::new(Arc::new(Store(PgMaintenanceStore::new(pool))));
    let modules = serde_json::from_value(json!([
        { "name": "orchestrator", "version": "1.2.0" },
        { "name": "engine-trino", "version": "0.3.0", "drain_path": "/drain" },
    ]))
    .unwrap();
    updates.set_modules(modules).await.unwrap();
    let body: ModuleClientsRequest = serde_json::from_value(json!({ "machine": {
        "capabilities": ["sql-query"], "audiences": ["core", "trino"], "secret": SECRET } }))
    .unwrap();
    provider
        .put_module_clients("orchestrator", &body, "https://qd.example.com")
        .await
        .unwrap();
    let app = api::platform_v1::routes(Some(provider.clone()), updates.clone());

    let core = token(&provider, api::platform_v1::AUDIENCE).await;
    let (status, body) = get(&app, Some(&core)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({ "modules": [
            { "name": "orchestrator", "version": "1.2.0" },
            { "name": "engine-trino", "version": "0.3.0" },
        ] })
    );

    let trino = token(&provider, "trino").await;
    for (who, token) in [
        ("another audience", Some(trino.as_str())),
        ("a forged token", Some("at+jwt|x.y.z")),
        ("no token", None),
    ] {
        let (status, body) = get(&app, token).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{who}");
        assert!(body.get("modules").is_none(), "{who}");
    }

    let without = api::platform_v1::routes(None, updates);
    let (status, _) = get(&without, Some(&core)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "no provider");
    drop(db);
}
