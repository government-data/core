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

/// The events and cursors, in memory.
#[derive(Default)]
struct Memory {
    events: Mutex<Vec<StoredEvent>>,
    cursors: Mutex<Vec<(Consumer, i64)>>,
}

#[async_trait]
impl EventsStore for Memory {
    async fn store(&self, publisher: &str, e: &NewEvent) -> Result<bool, String> {
        let mut events = self.events.lock().unwrap();
        if events.iter().any(|s| s.id == e.id) {
            return Ok(false);
        }
        let seq = i64::try_from(events.len()).unwrap() + 1;
        events.push(StoredEvent {
            seq,
            id: e.id.clone(),
            topic: e.topic.clone(),
            version: e.version,
            publisher: publisher.into(),
            occurred_at: e.occurred_at,
            data: e.data.clone(),
        });
        Ok(true)
    }

    async fn after(
        &self,
        topic: &str,
        version: u32,
        after: i64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, String> {
        let events = self.events.lock().unwrap();
        Ok(events
            .iter()
            .filter(|e| e.topic == topic && e.version == version && e.seq > after)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn cursor(&self, consumer: &Consumer) -> Result<Option<i64>, String> {
        let cursors = self.cursors.lock().unwrap();
        Ok(cursors.iter().find(|(c, _)| c == consumer).map(|(_, n)| *n))
    }

    async fn confirm(&self, consumer: &Consumer, cursor: i64) -> Result<(), String> {
        let mut cursors = self.cursors.lock().unwrap();
        match cursors.iter_mut().find(|(c, _)| c == consumer) {
            Some((_, n)) => *n = (*n).max(cursor),
            None => cursors.push((consumer.clone(), cursor)),
        }
        Ok(())
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

#[tokio::test]
async fn a_consumer_reads_after_its_cursor_and_confirms() {
    let (service, _, _) = service();
    for id in ["a", "b", "c"] {
        let e = event(id, FINISHED, 1, json!({}));
        service.publish("orchestrator", e).await.unwrap();
    }
    let now = async {};
    let batch = service.read("bi", FINISHED, 1, None, now).await.unwrap();
    assert_eq!(batch.events.len(), 3);
    assert_eq!(batch.cursor, 3);
    service.confirm("bi", FINISHED, 1, 2).await.unwrap();
    let batch = service
        .read("bi", FINISHED, 1, None, async {})
        .await
        .unwrap();
    assert_eq!(
        batch
            .events
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["c"]
    );
    service.confirm("bi", FINISHED, 1, 1).await.unwrap();
    let batch = service
        .read("bi", FINISHED, 1, Some(3), async {})
        .await
        .unwrap();
    assert!(batch.events.is_empty());
    assert_eq!(batch.cursor, 3, "a cursor never goes back");
    for (module, version) in [("orchestrator", 1), ("bi", 2)] {
        let refused = service
            .read(module, FINISHED, version, None, async {})
            .await;
        assert_eq!(refused, Err(EventsError::NotDeclared), "{module} {version}");
        let refused = service.confirm(module, FINISHED, version, 1).await;
        assert_eq!(refused, Err(EventsError::NotDeclared), "{module} {version}");
    }
}

#[tokio::test]
async fn a_read_with_none_waits_until_one_is_stored() {
    let (service, _, _) = service();
    let reader = service.clone();
    let read = tokio::spawn(async move {
        let forever = std::future::pending();
        reader.read("bi", FINISHED, 1, None, forever).await
    });
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    service.notified(Some("another.topic"));
    let e = event("a", FINISHED, 1, json!({}));
    service.publish("orchestrator", e).await.unwrap();
    service.notified(Some(FINISHED));
    let batch = read.await.unwrap().unwrap();
    assert_eq!(batch.events.len(), 1);
    assert_eq!(batch.cursor, 1);

    let waited = service
        .read(
            "bi",
            FINISHED,
            1,
            Some(1),
            tokio::time::sleep(std::time::Duration::from_millis(20)),
        )
        .await
        .unwrap();
    assert!(waited.events.is_empty(), "the deadline ends the wait");
    assert_eq!(waited.cursor, 1);
}
