use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::*;
use crate::events::EventSchema;
use crate::updates::tests::MemoryMaintenance;
use crate::updates::{InstalledModule, UpdatesError, UpdatesService};

/// Compiles every schema but one that holds `"bad": true`, and counts.
#[derive(Default)]
struct FakeSchemas(Mutex<usize>);

struct Any;

impl EventSchema for Any {
    fn validate(&self, _data: &Value) -> Result<(), Vec<String>> {
        Ok(())
    }
}

impl EventSchemas for FakeSchemas {
    fn compile(&self, schema: &Value) -> Result<Arc<dyn EventSchema>, String> {
        *self.0.lock().unwrap() += 1;
        if schema.get("bad").is_some() {
            return Err("bad keyword".into());
        }
        Ok(Arc::new(Any))
    }
}

fn provided(topic: &str, version: u32) -> ProvidedEvent {
    ProvidedEvent {
        topic: topic.into(),
        version,
        schema: json!({"type": "object"}),
    }
}

fn consumed(topic: &str, version: u32) -> ConsumedEvent {
    ConsumedEvent {
        topic: topic.into(),
        version,
    }
}

fn module(name: &str, events: ModuleEvents) -> InstalledModule {
    InstalledModule {
        name: name.into(),
        version: "1.0.0".into(),
        drain_path: None,
        route_path: None,
        menu: None,
        permissions: None,
        provides: Vec::new(),
        roles: Vec::new(),
        removal_path: None,
        events: Some(events),
    }
}

fn events(provides: Vec<ProvidedEvent>, consumes: Vec<ConsumedEvent>) -> ModuleEvents {
    ModuleEvents { provides, consumes }
}

#[test]
fn a_module_provides_its_own_topics_and_consumes_any_well_named_one() {
    let ok = events(
        vec![
            provided("orchestrator.run.finished", 1),
            provided("orchestrator.run.finished", 2),
        ],
        vec![
            consumed("core.iam.changed", 1),
            consumed("bi-superset.saved", 1),
        ],
    );
    assert_eq!(check("orchestrator", Some(&ok)), Ok(()));
    assert_eq!(check("orchestrator", None), Ok(()));
    for (module, topic) in [
        ("orchestrator", "core.iam.changed"),
        ("orchestrator", "other.run"),
        ("orchestrator", "orchestrator"),
        ("orchestrator", "orchestrator..run"),
        ("orchestrator", "orchestrator.Run"),
        ("orchestrator", "orchestrator.run-x"),
        ("core", "core.iam.changed"),
    ] {
        let bad = events(vec![provided(topic, 1)], Vec::new());
        assert_eq!(
            check(module, Some(&bad)),
            Err(Refusal::BadTopic(topic.into())),
            "{topic}"
        );
    }
    let bad = events(Vec::new(), vec![consumed("iam", 1)]);
    assert_eq!(
        check("orchestrator", Some(&bad)),
        Err(Refusal::BadTopic("iam".into()))
    );
}

#[test]
fn versions_counts_repeats_and_schema_sizes_are_checked() {
    let zero = events(Vec::new(), vec![consumed("core.iam.changed", 0)]);
    assert!(matches!(
        check("bi", Some(&zero)),
        Err(Refusal::BadVersion(_))
    ));
    let twice = events(vec![provided("bi.a", 1), provided("bi.a", 1)], Vec::new());
    assert!(matches!(
        check("bi", Some(&twice)),
        Err(Refusal::Repeated(_))
    ));
    let many = events(
        Vec::new(),
        (0..=MAX_EVENTS)
            .map(|i| consumed(&format!("core.t{i}"), 1))
            .collect(),
    );
    assert_eq!(check("bi", Some(&many)), Err(Refusal::TooMany));
    let mut big = provided("bi.a", 1);
    big.schema = json!({"description": "x".repeat(MAX_SCHEMA_BYTES)});
    let big = events(vec![big], Vec::new());
    assert!(matches!(
        check("bi", Some(&big)),
        Err(Refusal::BigSchema(_))
    ));
    let mut array = provided("bi.a", 1);
    array.schema = json!([1]);
    let array = events(vec![array], Vec::new());
    assert!(matches!(
        check("bi", Some(&array)),
        Err(Refusal::BigSchema(_))
    ));
}

#[tokio::test]
async fn schemas_are_compiled_when_the_list_is_told_and_one_that_does_not_keeps_the_list() {
    let schemas = Arc::new(FakeSchemas::default());
    let service = UpdatesService::new(Arc::new(MemoryMaintenance::default()))
        .with_event_schemas(schemas.clone());
    let kept = vec![module(
        "orchestrator",
        events(
            vec![provided("orchestrator.run.finished", 1)],
            vec![consumed("core.iam.changed", 1)],
        ),
    )];
    service.set_modules(kept.clone()).await.unwrap();
    assert_eq!(*schemas.0.lock().unwrap(), 1);
    assert_eq!(service.installed_modules().await.unwrap(), kept);
    let mut bad = provided("orchestrator.run.finished", 1);
    bad.schema = json!({"bad": true});
    let refused = service
        .set_modules(vec![module("orchestrator", events(vec![bad], Vec::new()))])
        .await;
    assert!(
        matches!(refused, Err(UpdatesError::Invalid("modules", ref why)) if why.contains("bad keyword"))
    );
    assert_eq!(service.installed_modules().await.unwrap(), kept);
}

#[tokio::test]
async fn without_a_compiler_a_module_that_provides_is_refused_and_one_that_consumes_is_not() {
    let service = UpdatesService::new(Arc::new(MemoryMaintenance::default()));
    let provides = module("bi", events(vec![provided("bi.a", 1)], Vec::new()));
    assert!(service.set_modules(vec![provides]).await.is_err());
    let consumes = module(
        "bi",
        events(Vec::new(), vec![consumed("core.iam.changed", 1)]),
    );
    assert!(service.set_modules(vec![consumes]).await.is_ok());
}

#[test]
fn the_list_reads_as_the_agent_sends_it() {
    let body = json!({
        "name": "orchestrator", "version": "2.0.0", "drain_path": null,
        "events": {
            "provides": [{"topic": "orchestrator.run.finished", "version": 1, "schema": {"type": "object"}}],
            "consumes": [{"topic": "core.iam.changed", "version": 1}]
        }
    });
    let m: InstalledModule = serde_json::from_value(body).unwrap();
    let e = m.events.unwrap();
    assert_eq!(e.provides, [provided("orchestrator.run.finished", 1)]);
    assert_eq!(e.consumes, [consumed("core.iam.changed", 1)]);
    let none: InstalledModule =
        serde_json::from_value(json!({"name": "docs", "version": "1.0.0"})).unwrap();
    assert!(none.events.is_none());
    assert!(serde_json::to_value(&none).unwrap().get("events").is_none());
}
