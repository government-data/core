//! A job's lines as `logging` writes them. Every value here is synthetic.

use std::io;
use std::sync::{Arc, Mutex};

use quality::queue::ClaimedJob;
use serde_json::{json, Map, Value};
use tracing::Instrument;
use uuid::Uuid;

use super::{blocking, span};

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn job(kind: &str, parameters: Value) -> ClaimedJob {
    ClaimedJob {
        tenant_id: Uuid::from_u128(1),
        id: Uuid::from_u128(0x10),
        kind: kind.to_string(),
        dataset: Some("orders".to_string()),
        parameters,
        engine: "duckdb".to_string(),
        attempts: 1,
    }
}

/// The lines `job` writes: one on the runtime, one on a blocking thread.
fn lines(job: &ClaimedJob) -> Vec<Map<String, Value>> {
    let out = Captured::default();
    let writer = out.clone();
    let subscriber = logging::json_subscriber("worker", "vm-test", move || writer.clone());
    tracing::subscriber::with_default(subscriber, || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(
            async {
                tracing::info!("job taken");
                blocking(|| tracing::info!(rows = 3_u64, "in the engine"))
                    .await
                    .unwrap();
            }
            .instrument(span(job)),
        );
    });
    let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    text.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn a_cut_jobs_lines_carry_its_job_id_and_run_id() {
    let cut = Uuid::from_u128(0x20);
    let lines = lines(&job(
        "evaluate",
        json!({ "cut": "2026-01-31", "cut_id": cut, "run": 2, "snapshot": "1" }),
    ));
    assert_eq!(lines.len(), 2, "{lines:?}");
    for line in &lines {
        assert_eq!(line["service"], "worker");
        assert_eq!(line["job_id"], Uuid::from_u128(0x10).to_string());
        assert_eq!(line["tenant"], Uuid::from_u128(1).to_string());
        assert_eq!(line["run_id"], cut.to_string());
        assert_eq!(line["run"], 2);
        assert_eq!(line["kind"], "evaluate");
        assert_eq!(line["dataset"], "orders");
        assert_eq!(line["attempt"], 1);
        assert!(!line.contains_key("job"), "{line:?}");
    }
    assert_eq!(lines[1]["msg"], "in the engine");
    assert_eq!(lines[1]["rows"], 3);
}

#[test]
fn a_job_of_no_cut_has_no_run_id() {
    let mut job = job("query", json!({ "sql": "SELECT 1" }));
    job.dataset = None;
    let lines = lines(&job);
    assert_eq!(lines.len(), 2, "{lines:?}");
    for line in &lines {
        assert_eq!(line["job_id"], Uuid::from_u128(0x10).to_string());
        assert!(!line.contains_key("run_id"), "{line:?}");
        assert!(!line.contains_key("dataset"), "{line:?}");
        assert!(!line.contains_key("sql"), "{line:?}");
    }
}

#[test]
fn a_cut_job_queued_before_run_was_given_has_only_its_run_id() {
    let cut = Uuid::from_u128(0x20);
    let lines = lines(&job(
        "extract",
        json!({ "cut": "2026-01-31", "cut_id": cut }),
    ));
    for line in &lines {
        assert_eq!(line["run_id"], cut.to_string());
        assert!(!line.contains_key("run"), "{line:?}");
    }
}

#[test]
fn a_cut_id_that_is_not_an_id_is_left_out() {
    let lines = lines(&job("extract", json!({ "cut_id": "not an id" })));
    assert!(lines.iter().all(|l| !l.contains_key("run_id")), "{lines:?}");
}
