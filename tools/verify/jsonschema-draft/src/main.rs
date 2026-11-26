//! Verification for decision 0017, section 5: the `jsonschema` crate, with
//! its default features off, validates module event schemas as Draft
//! 2020-12 and never fetches a remote `$ref`. Each step prints OK or FAIL;
//! the exit code is the number of failures. No network, no container.
//!
//! ```sh
//! cargo run -j 2 --manifest-path tools/verify/jsonschema-draft/Cargo.toml
//! ```

use jsonschema::{Draft, Retrieve, Uri};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};

static FETCHES: AtomicUsize = AtomicUsize::new(0);

/// A retriever that refuses every URI and counts the attempts.
struct Refuse;

impl Retrieve for Refuse {
    fn retrieve(
        &self,
        _uri: &Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        FETCHES.fetch_add(1, Ordering::SeqCst);
        Err("remote references are off".into())
    }
}

fn build(schema: &Value, refuse: bool) -> Result<jsonschema::Validator, String> {
    let mut o = jsonschema::options().with_draft(Draft::Draft202012);
    if refuse {
        o = o.with_retriever(Refuse);
    }
    o.build(schema).map_err(|e| e.to_string())
}

fn step(fails: &mut i32, name: &str, r: Result<String, String>) {
    match r {
        Ok(s) => println!("[OK  ] {name:<62} {s}"),
        Err(e) => {
            *fails += 1;
            println!("[FAIL] {name:<62} {e}");
        }
    }
}

fn event_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "urn:module:orchestrator:events:run-finished:1",
        "type": "object",
        "required": ["run_id", "steps"],
        "properties": {
            "run_id": {"$ref": "#/$defs/id"},
            "steps": {"type": "array", "prefixItems": [{"type": "string"}, {"type": "integer"}], "items": false}
        },
        "unevaluatedProperties": false,
        "$defs": {"id": {"type": "string", "minLength": 1}}
    })
}

fn draft_2020_12(f: &mut i32) {
    let v = build(&event_schema(), true);
    step(
        f,
        "2020-12 schema with $defs, local $ref, prefixItems compiles",
        v.as_ref().map(|_| "compiled".into()).map_err(Clone::clone),
    );
    let Ok(v) = v else { return };
    let ok = json!({"run_id": "r1", "steps": ["a", 2]});
    step(
        f,
        "a valid event validates",
        if v.is_valid(&ok) {
            Ok("valid".into())
        } else {
            Err("refused".into())
        },
    );
    let extra = json!({"run_id": "r1", "steps": ["a", 2], "other": 1});
    step(
        f,
        "unevaluatedProperties (2020-12) refuses an extra field",
        if v.is_valid(&extra) {
            Err("accepted".into())
        } else {
            Ok("refused".into())
        },
    );
    let third = json!({"run_id": "r1", "steps": ["a", 2, "x"]});
    step(
        f,
        "prefixItems + items:false (2020-12) refuses a third item",
        if v.is_valid(&third) {
            Err("accepted".into())
        } else {
            Ok("refused".into())
        },
    );
    let det = Draft::default().detect(&event_schema());
    step(
        f,
        "draft detected from $schema is 2020-12",
        match det {
            Draft::Draft202012 => Ok("Draft202012".into()),
            o => Err(format!("{o:?}")),
        },
    );
}

fn error_paths(f: &mut i32) {
    let Ok(v) = build(&event_schema(), true) else {
        return;
    };
    let secret = "person-name-should-not-appear";
    let bad = json!({"run_id": 7, "steps": [secret, 2]});
    let errs: Vec<_> = v.iter_errors(&bad).collect();
    let paths: Vec<String> = errs.iter().map(|e| e.schema_path().to_string()).collect();
    let at: Vec<String> = errs.iter().map(|e| e.instance_path().to_string()).collect();
    let named = !paths.is_empty() && paths.iter().all(|p| p.starts_with('/'));
    step(
        f,
        "errors carry a schema path and an instance path",
        if named {
            Ok(format!("{paths:?} at {at:?}"))
        } else {
            Err(format!("{paths:?}"))
        },
    );
    let leak = paths.iter().any(|p| p.contains(secret));
    step(
        f,
        "the schema path never carries the value",
        if leak {
            Err("value in path".into())
        } else {
            Ok("no value".into())
        },
    );
    let shown = errs.iter().any(|e| e.to_string().contains("7"));
    println!(
        "[INFO] {:<62} {}",
        "Display of an error quotes the instance value", shown
    );
}

fn remote_refs(f: &mut i32) {
    for (name, uri) in [
        (
            "https",
            "https://json-schema.org/draft/2020-12/schema#/$defs/x",
        ),
        ("http", "http://127.0.0.1:9/x.json"),
        ("file", "file:///etc/passwd"),
    ] {
        let s = json!({"$schema": "https://json-schema.org/draft/2020-12/schema", "$ref": uri});
        let other = json!({"$schema": "https://json-schema.org/draft/2020-12/schema", "properties": {"a": {"$ref": format!("{uri}-other")}}});
        let r = build(&s, false).and(build(&other, false));
        step(
            f,
            &format!("remote $ref ({name}) refused, default retriever"),
            match r {
                Ok(_) => Err("compiled".into()),
                Err(e) => Ok(e.chars().take(80).collect()),
            },
        );
        let before = FETCHES.load(Ordering::SeqCst);
        let r = build(&other, true);
        let n = FETCHES.load(Ordering::SeqCst) - before;
        step(
            f,
            &format!("remote $ref ({name}) refused, refusing retriever"),
            match r {
                Ok(_) => Err("compiled".into()),
                Err(_) => Ok(format!("{n} attempt(s), all refused")),
            },
        );
    }
    let meta = json!({"$schema": "https://json-schema.org/draft/2020-12/schema", "type": "object"});
    let before = FETCHES.load(Ordering::SeqCst);
    let r = build(&meta, true);
    let n = FETCHES.load(Ordering::SeqCst) - before;
    step(
        f,
        "the 2020-12 meta-schema is bundled (no fetch)",
        match r {
            Ok(_) if n == 0 => Ok("0 fetches".into()),
            Ok(_) => Err(format!("{n} fetches")),
            Err(e) => Err(e),
        },
    );
}

fn bad_schema(f: &mut i32) {
    let s = json!({"$schema": "https://json-schema.org/draft/2020-12/schema", "type": "nonsense"});
    step(
        f,
        "a schema that does not compile is an error",
        match build(&s, true) {
            Ok(_) => Err("compiled".into()),
            Err(_) => Ok("error".into()),
        },
    );
    let unknown = json!({"$schema": "https://example.com/my-draft", "type": "object"});
    step(
        f,
        "an unknown $schema with the draft forced",
        match build(&unknown, true) {
            Ok(_) => Ok("compiled as 2020-12".into()),
            Err(e) => Ok(format!("error: {}", e.chars().take(60).collect::<String>())),
        },
    );
}

fn main() {
    let mut f = 0;
    draft_2020_12(&mut f);
    error_paths(&mut f);
    remote_refs(&mut f);
    bad_schema(&mut f);
    println!("{} failure(s)", f);
    std::process::exit(f);
}
