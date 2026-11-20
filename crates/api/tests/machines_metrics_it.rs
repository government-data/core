//! `GET /machines/metrics` (0072, point 13, step 7) on PostgreSQL: a range
//! of the closed set only; Mimir not answering is 200 with
//! `available: false` and the range's window, never an error. The session
//! and `admin:ManageMachines` are the machines' router's, tested there.
//! Needs Docker.

use std::sync::Arc;

use application::MachinesService;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use infrastructure::mimir_series::MimirSeries;
use infrastructure::{PgAuditRepository, PgMachines, RingJoinTokenMint, SystemClock};
use serde_json::Value;
use tower::ServiceExt;

async fn get_json(app: &Router, uri: &str) -> (StatusCode, Value) {
    let request = Request::get(uri).body(Body::empty()).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20);
    (
        status,
        serde_json::from_slice(&body.await.unwrap()).unwrap(),
    )
}

#[tokio::test]
async fn mimir_down_is_available_false_and_a_range_is_one_of_three() {
    let db = testkit::spawn_test_db().await;
    let pool = db.pool.clone();
    // A port nothing listens on: Mimir down.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/prometheus", closed.local_addr().unwrap());
    drop(closed);
    let mimir = MimirSeries::plain(&base);
    let machines = MachinesService::new(
        Arc::new(PgMachines::new(pool.clone())),
        Arc::new(RingJoinTokenMint::new()),
        Arc::new(PgAuditRepository::new(pool)),
        Arc::new(SystemClock),
    )
    .with_machine_series(Arc::new(mimir));
    let app = Router::new()
        .route(
            "/machines/metrics",
            get(api::machines::metrics::metrics_handler),
        )
        .with_state(machines);

    for (uri, range, step, points) in [
        ("/machines/metrics", "24h", 600, 144),
        ("/machines/metrics?range=1h", "1h", 60, 60),
        ("/machines/metrics?range=7d", "7d", 3600, 168),
    ] {
        let (status, body) = get_json(&app, uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert_eq!(body["available"], false);
        assert_eq!(
            (body["range"].as_str(), body["step"].as_i64()),
            (Some(range), Some(step))
        );
        let (start, end) = (
            body["start"].as_i64().unwrap(),
            body["end"].as_i64().unwrap(),
        );
        assert_eq!((end - start) / step + 1, points);
        assert_eq!(end % step, 0, "the window ends on a step");
        assert_eq!(body["machines"], Value::Array(vec![]));
    }
    for uri in [
        "/machines/metrics?range=2h",
        "/machines/metrics?range=",
        "/machines/metrics?range=24h%20or%20up",
    ] {
        let (status, body) = get_json(&app, uri).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{uri}");
        assert!(body.to_string().contains("range"), "{body}");
    }
}
