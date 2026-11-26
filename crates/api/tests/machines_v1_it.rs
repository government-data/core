//! `GET /v1/machines` (quality-sdk decision 0011, ruling 56) end to end: a
//! module's machine client reads the installation's live machines, a
//! retired one left out; a call without the client's token is refused.
//! Needs Docker.

use std::sync::Arc;

use application::MachinesService;
use auth_gateway::{AuthorizationServer, ModuleClientsRequest, TokenRequest};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::oidc::{Ed25519Signer, PgAuthorizationCodes, PgOidcClients};
use infrastructure::{
    OAuthSecrets, PgAdminRepository, PgAuditRepository, PgMachines, ResetTokenAdapter,
    RingJoinTokenMint, SystemClock,
};
use serde_json::{json, Value};
use tower::ServiceExt;

const SECRET: &str = "machine-secret-0123456789abcde";

async fn get(app: &axum::Router, token: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::builder().uri("/v1/machines");
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

#[tokio::test]
async fn a_module_reads_the_live_machines() {
    let db = testkit::spawn_test_db().await;
    let pool = db.pool.clone();
    for (name, role, state) in [
        ("knime-1", "worker", "approved"),
        ("knime-2", "worker", "draining"),
        ("knime-3", "worker", "pending"),
        ("knime-old", "worker", "retired"),
        ("lake-a", "lake", "approved"),
    ] {
        sqlx::query("INSERT INTO machines (name, role, state) VALUES ($1, $2, $3)")
            .bind(name)
            .bind(role)
            .bind(state)
            .execute(&pool)
            .await
            .unwrap();
    }
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
    let body: ModuleClientsRequest = serde_json::from_value(json!({ "machine": {
        "capabilities": ["sql-query"], "audiences": ["core"], "secret": SECRET } }))
    .unwrap();
    provider
        .put_module_clients("orchestrator", &body, "https://qd.example.com")
        .await
        .unwrap();
    let request = TokenRequest {
        grant_type: Some("client_credentials".into()),
        client_id: Some("orchestrator-machine".into()),
        client_secret: Some(SECRET.into()),
        audience: Some(api::platform_v1::AUDIENCE.into()),
        ..TokenRequest::default()
    };
    let token = provider.token(&request).await.unwrap().access_token;
    let machines = MachinesService::new(
        Arc::new(PgMachines::new(pool.clone())),
        Arc::new(RingJoinTokenMint::new()),
        Arc::new(PgAuditRepository::new(pool.clone())),
        Arc::new(SystemClock),
    );
    let app = api::machines_v1::routes(Some(provider), machines);

    let (status, body) = get(&app, Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    let mut live: Vec<(String, String, String)> = body["machines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            let s = |k: &str| m[k].as_str().unwrap().to_owned();
            (s("name"), s("role"), s("state"))
        })
        .collect();
    live.sort();
    let want = [
        ("knime-1", "worker", "approved"),
        ("knime-2", "worker", "draining"),
        ("knime-3", "worker", "pending"),
        ("lake-a", "lake", "approved"),
    ]
    .map(|(n, r, s)| (n.to_owned(), r.to_owned(), s.to_owned()));
    assert_eq!(live, want, "every machine but the retired one");

    let (status, body) = get(&app, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.get("machines").is_none());
    drop(db);
}
