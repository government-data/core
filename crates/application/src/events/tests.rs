use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use serde_json::{json, Value};

use super::*;
use crate::updates::tests::MemoryMaintenance;
use crate::updates::{ConsumedEvent, InstalledModule, ModuleEvents, ProvidedEvent, UpdatesService};

/// Every schema compiles; `data` with `"bad"` fails at `/bad`. Counts the
/// compilations.
#[derive(Default)]
struct Schemas(Mutex<usize>);

struct NoBad;

impl EventSchema for NoBad {
    fn validate(&self, data: &Value) -> Result<(), Vec<String>> {
        match data.get("bad") {
            Some(_) => Err(vec!["/properties/bad".into()]),
            None => Ok(()),
        }
    }
}

impl EventSchemas for Schemas {
    fn compile(&self, _schema: &Value) -> Result<Arc<dyn EventSchema>, String> {
        *self.0.lock().unwrap() += 1;
        Ok(Arc::new(NoBad))
    }
}

/// The events, in memory: their ids and publishers.
#[derive(Default)]
struct Memory {
    events: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl EventsStore for Memory {
    async fn store(&self, publisher: &str, e: &NewEvent) -> Result<bool, String> {
        let mut events = self.events.lock().unwrap();
        if events.iter().any(|(id, _)| *id == e.id) {
            return Ok(false);
        }
        events.push((e.id.clone(), publisher.into()));
        Ok(true)
    }
}

fn module(name: &str, provides: &[(&str, u32)], consumes: &[(&str, u32)]) -> InstalledModule {
    let events = ModuleEvents {
        provides: provides
            .iter()
            .map(|(topic, version)| ProvidedEvent {
                topic: (*topic).into(),
                version: *version,
                schema: json!({ "type": "object", "title": topic }),
            })
            .collect(),
        consumes: consumes
            .iter()
            .map(|(topic, version)| ConsumedEvent {
                topic: (*topic).into(),
                version: *version,
            })
            .collect(),
    };
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

fn service() -> (EventsService, Arc<Memory>, Arc<Schemas>) {
    let updates = Arc::new(MemoryMaintenance::default());
    *updates.modules.lock().unwrap() = vec![
        module("orchestrator", &[("orchestrator.run.finished", 1)], &[]),
        module("bi", &[], &[("orchestrator.run.finished", 1)]),
    ];
    let store = Arc::new(Memory::default());
    let schemas = Arc::new(Schemas::default());
    let service = EventsService::new(store.clone(), UpdatesService::new(updates), schemas.clone());
    (service, store, schemas)
}

fn event(id: &str, topic: &str, version: u32, data: Value) -> NewEvent {
    NewEvent {
        id: id.into(),
        topic: topic.into(),
        version,
        occurred_at: Utc.with_ymd_and_hms(2026, 11, 25, 18, 0, 0).unwrap(),
        data,
    }
}

const FINISHED: &str = "orchestrator.run.finished";

#[tokio::test]
async fn a_module_publishes_its_declared_topic_once_per_id() {
    let (service, store, schemas) = service();
    let e = event("a", FINISHED, 1, json!({ "run_id": "r1" }));
    assert_eq!(
        service.publish("orchestrator", e.clone()).await,
        Ok(Published::Stored)
    );
    assert_eq!(
        service.publish("orchestrator", e).await,
        Ok(Published::Again)
    );
    let second = event("b", FINISHED, 1, json!({}));
    assert_eq!(
        service.publish("orchestrator", second).await,
        Ok(Published::Stored)
    );
    assert_eq!(store.events.lock().unwrap().len(), 2);
    assert_eq!(*schemas.0.lock().unwrap(), 1, "compiled once");
}

#[tokio::test]
async fn what_is_not_declared_too_big_or_invalid_is_refused() {
    let (service, store, _) = service();
    for (who, module, topic, version) in [
        ("another's topic", "bi", FINISHED, 1),
        ("a version not provided", "orchestrator", FINISHED, 2),
        ("a module not installed", "gone", FINISHED, 1),
    ] {
        let refused = service
            .publish(module, event("x", topic, version, json!({})))
            .await;
        assert_eq!(refused, Err(EventsError::NotDeclared), "{who}");
    }
    let big = json!({ "text": "x".repeat(MAX_DATA_BYTES) });
    let refused = service
        .publish("orchestrator", event("y", FINISHED, 1, big))
        .await;
    assert_eq!(refused, Err(EventsError::TooBig));
    let bad = event("z", FINISHED, 1, json!({ "bad": "a value" }));
    assert_eq!(
        service.publish("orchestrator", bad).await,
        Err(EventsError::Invalid(vec!["/properties/bad".into()]))
    );
    assert!(store.events.lock().unwrap().is_empty());
}
