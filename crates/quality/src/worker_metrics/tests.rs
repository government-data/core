use crate::queue_ready::KINDS;
use prometheus_client::registry::Registry;

use super::{kind_label, JobResult, WorkerMetrics, OTHER};

fn text(registry: &Registry) -> String {
    let mut text = String::new();
    prometheus_client::encoding::text::encode(&mut text, registry).unwrap();
    text
}

#[test]
fn every_label_is_a_kind_a_result_or_a_bucket() {
    let mut registry = Registry::default();
    let metrics = WorkerMetrics::register(&mut registry, 2);
    for (i, kind) in KINDS.iter().chain(&["no-such-kind"]).enumerate() {
        let job = metrics.started(kind);
        metrics.finished(job, JobResult::ALL[i % 3]);
    }
    metrics.set_claim_paused(true, false);
    let results: Vec<_> = JobResult::ALL.iter().map(|r| r.as_str()).collect();
    let samples = logging::metrics::samples(&text(&registry)).unwrap();
    for s in &samples {
        for (key, value) in &s.labels {
            match key.as_str() {
                "kind" => assert!(KINDS.contains(&value.as_str()) || value == OTHER),
                "result" => assert!(results.contains(&value.as_str())),
                "le" => assert!(s.name.ends_with("_bucket")),
                "reason" => assert!(["disk", "memory"].contains(&value.as_str())),
                other => panic!("label {other} on {}", s.name),
            }
        }
    }
    let names: std::collections::BTreeSet<_> = samples.iter().map(|s| s.name.as_str()).collect();
    let expected = [
        "qd_worker_slots",
        "qd_worker_jobs_running",
        "qd_worker_jobs_total",
        "qd_worker_job_duration_seconds_bucket",
        "qd_worker_job_duration_seconds_sum",
        "qd_worker_job_duration_seconds_count",
        "qd_worker_claim_paused",
    ];
    assert_eq!(names, expected.into_iter().collect());
}

#[test]
fn running_and_slots_follow_the_jobs() {
    let mut registry = Registry::default();
    let metrics = WorkerMetrics::register(&mut registry, 3);
    let a = metrics.started("evaluate");
    let b = metrics.started("extract");
    let value = |name: &str| {
        logging::metrics::samples(&text(&registry))
            .unwrap()
            .into_iter()
            .find(|s| s.name == name)
            .map(|s| s.value)
    };
    assert_eq!(value("qd_worker_slots"), Some(3.0));
    assert_eq!(value("qd_worker_jobs_running"), Some(2.0));
    metrics.finished(a, JobResult::Ok);
    metrics.finished(b, JobResult::Failed);
    assert_eq!(value("qd_worker_jobs_running"), Some(0.0));
    let text = text(&registry);
    assert!(text.contains("qd_worker_jobs_total{kind=\"evaluate\",result=\"ok\"} 1"));
    assert!(text.contains("qd_worker_jobs_total{kind=\"extract\",result=\"failed\"} 1"));
}

#[test]
fn a_kind_outside_the_queue_list_is_other() {
    assert_eq!(kind_label("evaluate"), "evaluate");
    assert_eq!(kind_label("dataset-from-a-tenant"), OTHER);
}
