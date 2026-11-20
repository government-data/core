//! The lines as they are written, captured from a subscriber with each layer.
//! Every value here is synthetic.

use std::io;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tracing_subscriber::layer::SubscriberExt;

use super::{json_layer, pretty_layer, REDACTED};

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

impl Captured {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

/// The JSON lines `emit` writes, as text.
fn json_text(emit: impl FnOnce()) -> String {
    let out = Captured::default();
    let writer = out.clone();
    let layer = json_layer("server", "vm-test", move || writer.clone());
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), emit);
    out.text()
}

/// The one JSON line `emit` writes, parsed.
fn json_line(emit: impl FnOnce()) -> serde_json::Map<String, Value> {
    let text = json_text(emit);
    assert_eq!(text.lines().count(), 1, "{text}");
    match serde_json::from_str(&text).unwrap() {
        Value::Object(object) => object,
        other => panic!("not an object: {other}"),
    }
}

fn pretty_text(emit: impl FnOnce()) -> String {
    let out = Captured::default();
    let writer = out.clone();
    let layer = pretty_layer(move || writer.clone());
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), emit);
    out.text()
}

#[test]
fn a_line_is_one_flat_object_with_the_reserved_keys_first() {
    let text = json_text(|| tracing::info!(rows = 3_u64, "loaded"));
    assert!(text.starts_with(r#"{"time":""#), "{text}");
    assert!(text.ends_with("}\n"), "{text}");
    assert_eq!(text.lines().count(), 1, "{text}");
    let line: serde_json::Map<String, Value> = serde_json::from_str(&text).unwrap();
    assert!(
        line.values().all(|v| !v.is_object() && !v.is_array()),
        "{text}"
    );
    assert_eq!(line["level"], "INFO");
    assert_eq!(line["service"], "server");
    assert_eq!(line["vm"], "vm-test");
    assert_eq!(line["target"], module_path!());
    assert_eq!(line["msg"], "loaded");
    assert_eq!(line["rows"], 3);
    assert!(line["time"].as_str().unwrap().ends_with('Z'));
    for old in ["timestamp", "fields", "span", "spans"] {
        assert!(!line.contains_key(old), "{old} in {line:?}");
    }
}

#[test]
fn ids_are_lifted_from_the_spans_in_scope() {
    let line = json_line(|| {
        let outer = tracing::info_span!("job", job_id = "job-1", run_id = "run-1", run = 2_u64);
        let _outer = outer.enter();
        let inner = tracing::info_span!("request", request_id = "req-1", tenant = "tenant-1");
        let _inner = inner.enter();
        tracing::info!("inside");
    });
    assert_eq!(line["job_id"], "job-1");
    assert_eq!(line["run_id"], "run-1");
    assert_eq!(line["request_id"], "req-1");
    assert_eq!(line["tenant"], "tenant-1");
    assert_eq!(line["run"], 2);
}

#[test]
fn absent_ids_are_absent_not_null() {
    let line = json_line(|| tracing::warn!("alone"));
    for id in ["tenant", "request_id", "run_id", "job_id"] {
        assert!(!line.contains_key(id), "{id} in {line:?}");
    }
    assert_eq!(line["level"], "WARN");
}

#[test]
fn the_event_wins_over_a_span_and_the_inner_span_over_the_outer() {
    let line = json_line(|| {
        let outer = tracing::info_span!("a", job_id = "outer", kind = "outer", depth = 1_u64);
        let _outer = outer.enter();
        let inner = tracing::info_span!("b", kind = "inner");
        let _inner = inner.enter();
        tracing::info!(job_id = "event", "collide");
    });
    assert_eq!(line["job_id"], "event");
    assert_eq!(line["kind"], "inner");
    assert_eq!(line["depth"], 1);
}

#[test]
fn a_field_named_like_a_reserved_key_is_kept_apart() {
    let line = json_line(|| tracing::info!(level = "custom", service = "other", "reserved"));
    assert_eq!(line["level"], "INFO");
    assert_eq!(line["service"], "server");
    assert_eq!(line["field.level"], "custom");
    assert_eq!(line["field.service"], "other");
}

#[test]
fn numbers_and_booleans_keep_their_types() {
    let line = json_line(
        || tracing::info!(status = 200_u16, delta = -4_i64, ratio = 0.5, ok = true, name = %"x", "typed"),
    );
    assert_eq!(line["status"], Value::from(200));
    assert_eq!(line["delta"], Value::from(-4));
    assert_eq!(line["ratio"], Value::from(0.5));
    assert_eq!(line["ok"], Value::from(true));
    assert_eq!(line["name"], Value::from("x"));
}

#[test]
fn the_request_line_has_its_id_and_status_at_the_top_level() {
    let text = json_text(|| {
        let span = tracing::info_span!("request", method = %"GET", path = %"/health", request_id = %"0123abcd");
        let _span = span.enter();
        tracing::info!(
            status = 200_u16,
            duration_ms = 1.5_f64,
            bytes = 2_u64,
            "request completed"
        );
    });
    // What the Loki datasource's derived field matches: no space after the colon.
    assert!(text.contains(r#""request_id":"0123abcd""#), "{text}");
    let line: serde_json::Map<String, Value> = serde_json::from_str(&text).unwrap();
    assert_eq!(line["status"], 200);
    assert_eq!(line["duration_ms"], 1.5);
    assert_eq!(line["bytes"], 2);
    assert_eq!(line["path"], "/health");
}

#[test]
fn sensitive_fields_are_redacted_in_json() {
    let line = json_line(|| {
        let span = tracing::info_span!("call", authorization = "Bearer synthetic", job_id = "j");
        let _span = span.enter();
        tracing::info!(
            password = "synthetic-pw",
            client_secret = "synthetic-secret",
            db.statement = "SELECT 1",
            email = "someone@example.invalid",
            tokens_used = 7_u64,
            "redacted"
        );
    });
    for key in [
        "password",
        "client_secret",
        "db.statement",
        "email",
        "authorization",
    ] {
        assert_eq!(line[key], REDACTED, "{key}");
    }
    assert_eq!(line["tokens_used"], 7);
    assert_eq!(line["job_id"], "j");
}

#[test]
fn a_field_recorded_later_on_a_span_is_redacted_too() {
    let text = json_text(|| {
        let span = tracing::info_span!("call", token = tracing::field::Empty);
        span.record("token", "synthetic-token");
        let _span = span.enter();
        tracing::info!("later");
    });
    assert!(!text.contains("synthetic-token"), "{text}");
    assert!(text.contains(REDACTED), "{text}");
}

#[test]
fn sensitive_fields_are_redacted_in_pretty() {
    let text = pretty_text(|| {
        let span = tracing::info_span!("call", cookie = "synthetic-cookie");
        let _span = span.enter();
        tracing::info!(api_key = "synthetic-key", rows = 3_u64, "pretty");
    });
    assert!(!text.contains("synthetic-cookie"), "{text}");
    assert!(!text.contains("synthetic-key"), "{text}");
    assert!(text.contains(REDACTED), "{text}");
    assert!(text.contains("pretty"), "{text}");
    assert!(text.contains('3'), "{text}");
}

#[test]
fn the_rule_takes_whole_words_at_the_end_of_a_name() {
    for name in [
        "password",
        "PASSWORD",
        "access_token",
        "set-cookie",
        "x.authorization",
        "db.statement",
    ] {
        assert!(super::is_sensitive(name), "{name}");
    }
    for name in [
        "job_id",
        "tokens_used",
        "secret_count",
        "statement",
        "mailbox",
        "key",
    ] {
        assert!(!super::is_sensitive(name), "{name}");
    }
}

/// Panics with a synthetic message inside a job's span, caught, as the
/// hook [`super::init`] installs sees it; returns the line it was raised on.
fn panic_in_a_job() -> u32 {
    super::panic::install();
    let span = tracing::info_span!("job", job_id = "job-test", run_id = "run-test");
    let _span = span.enter();
    let line = line!() + 1;
    let caught = std::panic::catch_unwind(|| panic!("synthetic-payload-7f3a"));
    assert!(caught.is_err());
    line
}

#[test]
fn a_panic_is_one_error_line_with_its_location_and_not_its_message() {
    let mut line = 0;
    let lines = super::json_lines("worker", || line = panic_in_a_job());
    assert_eq!(lines.len(), 1, "{lines:?}");
    let panic = &lines[0];
    assert_eq!(panic["level"], "ERROR");
    assert_eq!(panic["msg"], "panic");
    assert_eq!(panic["service"], "worker");
    assert_eq!(panic["job_id"], "job-test");
    assert_eq!(panic["run_id"], "run-test");
    let location = panic["location"].as_str().unwrap();
    assert!(
        location.ends_with(&format!("tests.rs:{line}")),
        "{location}"
    );
    assert!(!panic["thread"].as_str().unwrap().is_empty());
    let text = serde_json::to_string(panic).unwrap();
    assert!(!text.contains("synthetic-payload"), "{text}");
    assert!(!text.contains("backtrace"), "{text}");
}

#[test]
fn a_panic_is_logged_in_pretty_too() {
    let text = pretty_text(|| {
        panic_in_a_job();
    });
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(text.contains("ERROR"), "{text}");
    assert!(text.contains("panic"), "{text}");
    assert!(text.contains("location"), "{text}");
    assert!(!text.contains("synthetic-payload"), "{text}");
}
