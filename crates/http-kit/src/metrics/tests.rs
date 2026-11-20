use std::collections::BTreeSet;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use prometheus_client::registry::Registry;
use tower::ServiceExt;

use super::{status_class, track, HttpMetrics, STATUS_CLASSES, UNMATCHED};

/// A router like the API's: a template with an id, a nested one, a route
/// that answers 404 itself, and the fallback.
fn app(metrics: HttpMetrics) -> Router {
    let datasets = Router::new().route("/{dataset}/fields/{field}", get(|| async { "field" }));
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/datasets/{id}", get(|| async { StatusCode::NOT_FOUND }))
        .nest("/catalog", datasets)
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(axum::middleware::from_fn_with_state(metrics, track))
}

async fn get_path(app: &Router, path: &str) -> StatusCode {
    let request = Request::get(path).body(Body::empty()).unwrap();
    app.clone().oneshot(request).await.unwrap().status()
}

fn text(registry: &Registry) -> String {
    let mut text = String::new();
    prometheus_client::encoding::text::encode(&mut text, registry).unwrap();
    text
}

/// Every value the `route` label takes in the registry's text.
fn routes(text: &str) -> BTreeSet<String> {
    logging::metrics::samples(text)
        .unwrap()
        .iter()
        .filter_map(|s| s.label("route").map(str::to_string))
        .collect()
}

#[tokio::test]
async fn a_route_is_labelled_by_its_template_and_an_unknown_path_is_unmatched() {
    let mut registry = Registry::default();
    let app = app(HttpMetrics::register(&mut registry));
    // Synthetic ids and an unknown path, none of which may become a label.
    let sent = [
        "/health",
        "/datasets/123",
        "/datasets/0b5a7c1e-0000-4000-8000-000000000001",
        "/catalog/members-a/fields/zone-b",
        "/catalog/members-c/fields/zone-d",
        "/no/such/path/42",
        "/datasets/123/extra",
    ];
    for path in sent {
        get_path(&app, path).await;
    }
    let text = text(&registry);
    let expected: BTreeSet<String> = [
        "/health",
        "/datasets/{id}",
        "/catalog/{dataset}/fields/{field}",
        UNMATCHED,
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(routes(&text), expected, "{text}");
    for value in ["123", "0b5a7c1e", "members", "zone", "/no/such", "extra"] {
        assert!(
            !routes(&text).iter().any(|r| r.contains(value)),
            "{value} in a label"
        );
    }
}

#[tokio::test]
async fn more_requests_to_new_ids_add_no_series() {
    let mut registry = Registry::default();
    let app = app(HttpMetrics::register(&mut registry));
    get_path(&app, "/datasets/1").await;
    get_path(&app, "/nowhere/1").await;
    let before = logging::metrics::samples(&text(&registry)).unwrap().len();
    for n in 2..50 {
        get_path(&app, &format!("/datasets/{n}")).await;
        get_path(&app, &format!("/nowhere/{n}")).await;
    }
    let after = logging::metrics::samples(&text(&registry)).unwrap();
    assert_eq!(after.len(), before);
    let count = after
        .iter()
        .find(|s| s.name == "qd_http_requests_total" && s.label("route") == Some("/datasets/{id}"))
        .unwrap();
    assert!((count.value - 49.0).abs() < f64::EPSILON);
}

#[tokio::test]
async fn every_label_is_a_route_a_class_or_a_bucket() {
    let mut registry = Registry::default();
    let app = app(HttpMetrics::register(&mut registry));
    assert_eq!(get_path(&app, "/health").await, StatusCode::OK);
    assert_eq!(get_path(&app, "/datasets/7").await, StatusCode::NOT_FOUND);
    for s in logging::metrics::samples(&text(&registry)).unwrap() {
        for (key, value) in &s.labels {
            match key.as_str() {
                "route" => assert!(value.starts_with('/') || value == UNMATCHED),
                "status_class" => assert!(STATUS_CLASSES.contains(&value.as_str())),
                "le" => assert!(s.name.ends_with("_bucket")),
                other => panic!("label {other} on {}", s.name),
            }
        }
        match s.name.as_str() {
            "qd_http_requests_total" => assert_eq!(s.labels.len(), 2),
            "qd_http_request_duration_seconds_bucket" => assert_eq!(s.labels.len(), 2),
            "qd_http_request_duration_seconds_sum" | "qd_http_request_duration_seconds_count" => {
                assert_eq!(s.labels.len(), 1);
            }
            other => panic!("unexpected metric {other}"),
        }
    }
    let text = text(&registry);
    assert!(text.contains("route=\"/datasets/{id}\",status_class=\"4xx\""));
    assert!(text.contains("route=\"/health\",status_class=\"2xx\""));
}

#[test]
fn status_classes_are_the_fixed_list() {
    assert_eq!(status_class(101), "1xx");
    assert_eq!(status_class(204), "2xx");
    assert_eq!(status_class(304), "3xx");
    assert_eq!(status_class(429), "4xx");
    assert_eq!(status_class(503), "5xx");
    assert_eq!(status_class(999), "other");
}
