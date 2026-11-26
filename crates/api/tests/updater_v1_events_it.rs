//! `PUT /updater/v1/modules` with a module's events (quality-sdk decision
//! 0017): the core keeps the declarations with the list, compiles each
//! schema with the real adapter, and a schema that does not compile, or a
//! topic that is not the module's, refuses the list and keeps the one
//! before. The version names the feature. Needs Docker.

use std::sync::Arc;

mod machine_certs;

use application::{Installed, MachinesService, UpdatesService};
use auth_gateway::http::client_cert::ClientCertificates;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::ca::{Ca, X509CertificateReader, DEFAULT_CA_VALIDITY};
use infrastructure::event_schemas::JsonSchemas;
use infrastructure::{
    PgAuditRepository, PgMachines, PgMaintenanceStore, RingJoinTokenMint, SystemClock,
};
use machine_certs::{csr, key, origin, traefik_header};
use serde_json::{json, Value};
use tower::ServiceExt;

#[path = "../../server/src/updates_wiring.rs"]
mod updates_wiring;
use updates_wiring::Store;

async fn call(
    app: &axum::Router,
    cert: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("x-forwarded-tls-client-cert", traefik_header(&[cert]))
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

fn orchestrator(schema: Value, topic: &str) -> Value {
    json!({"modules": [{
        "name": "orchestrator", "version": "2.0.0", "drain_path": null,
        "events": {
            "provides": [{"topic": topic, "version": 1, "schema": schema}],
            "consumes": [{"topic": "core.iam.changed", "version": 1}]
        }
    }]})
}

#[tokio::test]
async fn a_module_s_events_are_kept_with_the_list_and_a_bad_schema_keeps_the_one_before() {
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
    let updates = UpdatesService::new(Arc::new(Store(PgMaintenanceStore::new(pool))))
        .with_event_schemas(Arc::new(JsonSchemas));
    let certificates = ClientCertificates::trusting(Arc::new(X509CertificateReader));
    let app = api::updater_v1::routes(updates.clone(), machines.clone(), certificates);
    let Installed::Signed { certificate, .. } = machines
        .install_updater("updater-1", &csr(&key()), None, &origin())
        .await
        .unwrap()
    else {
        panic!("signed");
    };
    let cert = certificate.pem.as_str();

    let (status, version) = call(&app, cert, "GET", "/updater/v1/version", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(version["features"]
        .as_array()
        .unwrap()
        .contains(&json!("module-events")));

    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object", "properties": {"run_id": {"type": "string"}}
    });
    let body = orchestrator(schema.clone(), "orchestrator.run.finished");
    let (status, _) = call(&app, cert, "PUT", "/updater/v1/modules", Some(body)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let kept = updates.installed_modules().await.unwrap();
    let events = kept[0].events.as_ref().unwrap();
    assert_eq!(events.provides[0].schema, schema);
    assert_eq!(events.consumes[0].topic, "core.iam.changed");

    let remote = json!({"$ref": "https://example.com/s.json"});
    for body in [
        orchestrator(remote, "orchestrator.run.finished"),
        orchestrator(json!({"type": "nonsense"}), "orchestrator.run.finished"),
        orchestrator(schema, "core.iam.changed"),
    ] {
        let (status, answer) = call(&app, cert, "PUT", "/updater/v1/modules", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
        assert_eq!(updates.installed_modules().await.unwrap(), kept);
    }
}
