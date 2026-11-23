//! A line logged inside a job's route, as `logging` writes it. Every value
//! here is synthetic.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::Router;
use serde_json::{json, Map, Value};
use tower::ServiceExt;
use tracing::Instrument;
use uuid::Uuid;

use super::{cut_of, enter, record};

const TENANT: Uuid = Uuid::from_u128(0xa);
const JOB: Uuid = Uuid::from_u128(0x10);
const CUT: Uuid = Uuid::from_u128(0x20);

/// The lines of one call to a job's report route whose job has
/// `parameters`, inside a `request` span as the API's.
fn lines(uri: &str, parameters: Value) -> Vec<Map<String, Value>> {
    let handler = move || async move {
        record("extract", Some("orders"), &parameters);
        tracing::info!("in the handler");
        StatusCode::NO_CONTENT
    };
    let app = Router::new()
        .route("/internal/jobs/{tenant}/{id}/report", post(handler.clone()))
        .route("/internal/jobs/{id}/report", post(handler))
        .route_layer(axum::middleware::from_fn(enter));
    let request = Request::post(uri).body(Body::empty()).unwrap();
    logging::json_lines("server", || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let served = app
            .oneshot(request)
            .instrument(tracing::info_span!("request", request_id = "0123abcd"));
        runtime.block_on(served).unwrap();
    })
}

#[test]
fn a_line_in_a_cut_jobs_route_carries_its_ids_at_the_top_level() {
    let uri = format!("/internal/jobs/{TENANT}/{JOB}/report");
    let parameters = json!({ "cut": "2026-01-31", "cut_id": CUT, "run": 2 });
    let lines = lines(&uri, parameters);
    assert_eq!(lines.len(), 1, "{lines:?}");
    let line = &lines[0];
    assert_eq!(line["job_id"], JOB.to_string());
    assert_eq!(line["tenant"], TENANT.to_string());
    assert_eq!(line["run_id"], CUT.to_string());
    assert_eq!(line["run"], 2);
    assert_eq!(line["kind"], "extract");
    assert_eq!(line["dataset"], "orders");
    assert_eq!(line["request_id"], "0123abcd");
    // Only ids: no other value of the parameters.
    assert!(!line.contains_key("cut"), "{line:?}");
}

#[test]
fn a_job_of_no_cut_has_its_job_id_and_tenant_only() {
    let uri = format!("/internal/jobs/{TENANT}/{JOB}/report");
    let lines = lines(&uri, json!({ "source": "ledger" }));
    let line = &lines[0];
    assert_eq!(line["job_id"], JOB.to_string());
    assert_eq!(line["tenant"], TENANT.to_string());
    assert!(!line.contains_key("run_id"), "{line:?}");
    assert!(!line.contains_key("run"), "{line:?}");
    assert!(!line.contains_key("source"), "{line:?}");
}

#[test]
fn a_route_without_the_tenant_has_the_job_id_only() {
    let uri = format!("/internal/jobs/{JOB}/report");
    let lines = lines(&uri, json!({ "source": "ledger" }));
    let line = &lines[0];
    assert_eq!(line["job_id"], JOB.to_string());
    assert!(!line.contains_key("tenant"), "{line:?}");
}

#[test]
fn a_path_that_is_not_ids_opens_no_span() {
    let lines = lines("/internal/jobs/a/b/report", json!({}));
    assert!(lines.iter().all(|l| !l.contains_key("job_id")), "{lines:?}");
}

#[test]
fn a_cut_job_queued_before_run_was_given_has_only_its_id() {
    assert_eq!(cut_of(&json!({ "cut_id": CUT })), (Some(CUT), None));
    assert_eq!(cut_of(&json!({ "cut_id": "x", "run": "2" })), (None, None));
    assert_eq!(
        cut_of(&json!({ "cut_id": CUT, "run": 3 })),
        (Some(CUT), Some(3))
    );
}
