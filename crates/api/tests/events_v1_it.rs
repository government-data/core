//! `/v1/events` (quality-sdk decision 0017) end to end on PostgreSQL: a
//! module publishes its declared topic with its machine client, validated
//! by the real schema adapter and stored once per id; another module may
//! not publish it. Needs Docker.

use std::sync::Arc;

use application::events::EventsService;
use application::UpdatesService;
use auth_gateway::{AuthorizationServer, ModuleClientsRequest, TokenRequest};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::event_schemas::JsonSchemas;
use infrastructure::module_events_repo::PgModuleEvents;
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
const TOPIC: &str = "orchestrator.run.finished";
const ID: &str = "0193a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";

async fn call(
    app: &axum::Router,
    token: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json");
    let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
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

async fn client(provider: &AuthorizationServer, module: &str) -> String {
    let secret = format!("{SECRET}-{module}");
    let body: ModuleClientsRequest = serde_json::from_value(json!({ "machine": {
        "capabilities": ["sql-query"], "audiences": ["core"], "secret": secret } }))
    .unwrap();
    provider
        .put_module_clients(module, &body, "https://qd.example.com")
        .await
        .unwrap();
    let request = TokenRequest {
        grant_type: Some("client_credentials".into()),
        client_id: Some(format!("{module}-machine")),
        client_secret: Some(secret),
        audience: Some(api::platform_v1::AUDIENCE.into()),
        ..TokenRequest::default()
    };
    provider.token(&request).await.unwrap().access_token
}

fn modules() -> Vec<application::InstalledModule> {
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object", "required": ["run_id"],
        "properties": { "run_id": { "type": "string" } },
        "additionalProperties": false
    });
    serde_json::from_value(json!([
        { "name": "orchestrator", "version": "1.0.0", "events": {
            "provides": [{ "topic": TOPIC, "version": 1, "schema": schema }] } },
        { "name": "bi-superset", "version": "1.0.0", "events": {
            "consumes": [{ "topic": TOPIC, "version": 1 }] } },
    ]))
    .unwrap()
}

fn event(id: &str, data: Value) -> Value {
    json!({ "id": id, "topic": TOPIC, "version": 1,
        "occurred_at": "2026-11-25T18:00:00Z", "data": data })
}

#[tokio::test]
async fn a_module_publishes_its_topic_once_per_id_and_another_may_not() {
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
    let updates = UpdatesService::new(Arc::new(Store(PgMaintenanceStore::new(pool.clone()))))
        .with_event_schemas(Arc::new(JsonSchemas));
    updates.set_modules(modules()).await.unwrap();
    let events = EventsService::new(
        Arc::new(PgModuleEvents::new(pool.clone())),
        updates,
        Arc::new(JsonSchemas),
    );
    let app = api::events_v1::routes(Some(provider.clone()), events);
    let orchestrator = client(&provider, "orchestrator").await;
    let bi = client(&provider, "bi-superset").await;

    let first = event(ID, json!({ "run_id": "r1" }));
    let (status, _) = call(
        &app,
        &orchestrator,
        "POST",
        "/v1/events",
        Some(first.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = call(&app, &orchestrator, "POST", "/v1/events", Some(first)).await;
    assert_eq!(status, StatusCode::OK, "the same id again");

    let (status, body) = call(&app, &bi, "POST", "/v1/events", Some(event(ID, json!({})))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "not bi's topic: {body}");
    let bad = event(
        "0193a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5c",
        json!({ "run_id": 7 }),
    );
    let (status, body) = call(&app, &orchestrator, "POST", "/v1/events", Some(bad)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["schema_paths"], json!(["/properties/run_id/type"]));
    assert!(!body.to_string().contains('7'), "never the value: {body}");
    let not_uuid = event("r1", json!({ "run_id": "r1" }));
    let (status, _) = call(&app, &orchestrator, "POST", "/v1/events", Some(not_uuid)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let stored: Vec<(String, String, Value)> =
        sqlx::query_as("SELECT id::text, publisher, data FROM module_events ORDER BY seq")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        stored,
        [(
            ID.to_owned(),
            "orchestrator".to_owned(),
            json!({ "run_id": "r1" })
        )],
        "stored once, with its publisher"
    );
    drop(db);
}
