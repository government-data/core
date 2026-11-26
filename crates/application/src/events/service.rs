//! The module events' use case (quality-sdk decision 0017): a module
//! publishes only the topics and versions it provides, each event
//! validated against the schema it declares; it reads and confirms only
//! the topics it consumes. What a module declares is read from the
//! installed modules as the update agent last told them, so a version
//! taken out of a module's `provides` stops being accepted at once.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::broadcast;

use super::store::{Consumer, EventsStore, NewEvent, StoredEvent};
use super::{EventSchema, EventSchemas};
use crate::updates::{InstalledModule, UpdatesService};

/// The largest `data`, as JSON text.
pub const MAX_DATA_BYTES: usize = 256 * 1024;
/// The most events one read answers.
pub const MAX_READ: usize = 100;

/// Why an event is not taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventsError {
    /// The caller's module does not provide (or consume) the topic at that
    /// version.
    NotDeclared,
    /// `data` is larger than [`MAX_DATA_BYTES`].
    TooBig,
    /// `data` does not validate: the schema paths it fails at, never the
    /// values.
    Invalid(Vec<String>),
    /// The declared schema does not compile any longer.
    BadSchema,
    /// The database did not answer.
    Unavailable(String),
}

/// What a publish did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Published {
    Stored,
    /// The same id was already stored.
    Again,
}

/// A read: the events, and the cursor to confirm once they are handled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub events: Vec<StoredEvent>,
    pub cursor: i64,
}

type Compiled = HashMap<(String, String, u32), (Value, Arc<dyn EventSchema>)>;

/// Publishes, reads and confirms the modules' events.
#[derive(Clone)]
pub struct EventsService {
    store: Arc<dyn EventsStore>,
    updates: UpdatesService,
    schemas: Arc<dyn EventSchemas>,
    /// Each declared schema compiled once, by module, topic and version,
    /// with the schema it was compiled from.
    compiled: Arc<Mutex<Compiled>>,
    /// The topics stored since, to wake the reads that wait; an empty one
    /// wakes every read (the listener reconnected).
    wake: broadcast::Sender<String>,
}

impl std::fmt::Debug for EventsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventsService").finish_non_exhaustive()
    }
}

impl EventsService {
    pub fn new(
        store: Arc<dyn EventsStore>,
        updates: UpdatesService,
        schemas: Arc<dyn EventSchemas>,
    ) -> Self {
        Self {
            store,
            updates,
            schemas,
            compiled: Arc::default(),
            wake: broadcast::channel(256).0,
        }
    }

    /// An event of `topic` was stored (by this replica or another); `None`
    /// when what was stored meanwhile is not known.
    pub fn notified(&self, topic: Option<&str>) {
        let _ = self.wake.send(topic.unwrap_or_default().to_owned());
    }

    /// Publish `event` from `module`.
    pub async fn publish(&self, module: &str, event: NewEvent) -> Result<Published, EventsError> {
        let size = serde_json::to_vec(&event.data).map_or(usize::MAX, |b| b.len());
        if size > MAX_DATA_BYTES {
            return Err(EventsError::TooBig);
        }
        let installed = self.installed(module).await?;
        let schema = installed
            .and_then(|m| m.events)
            .and_then(|e| {
                e.provides
                    .into_iter()
                    .find(|p| p.topic == event.topic && p.version == event.version)
            })
            .ok_or(EventsError::NotDeclared)?
            .schema;
        self.schema(module, &event.topic, event.version, schema)?
            .validate(&event.data)
            .map_err(EventsError::Invalid)?;
        match self.store.store(module, &event).await {
            Ok(true) => Ok(Published::Stored),
            Ok(false) => Ok(Published::Again),
            Err(e) => Err(EventsError::Unavailable(e)),
        }
    }

    /// `module`'s installed entry, `None` when it is not installed.
    async fn installed(&self, module: &str) -> Result<Option<InstalledModule>, EventsError> {
        let modules = self
            .updates
            .installed_modules()
            .await
            .map_err(EventsError::Unavailable)?;
        Ok(modules.into_iter().find(|m| m.name == module))
    }

    /// The compiled `schema`, compiled now if it was not or it changed.
    fn schema(
        &self,
        module: &str,
        topic: &str,
        version: u32,
        schema: Value,
    ) -> Result<Arc<dyn EventSchema>, EventsError> {
        let key = (module.to_owned(), topic.to_owned(), version);
        let held = self.lock().get(&key).cloned();
        if let Some((from, compiled)) = held {
            if from == schema {
                return Ok(compiled);
            }
        }
        let compiled = self.schemas.compile(&schema).map_err(|e| {
            tracing::error!(
                module,
                topic,
                version,
                "a declared event schema does not compile: {e}"
            );
            EventsError::BadSchema
        })?;
        self.lock().insert(key, (schema, compiled.clone()));
        Ok(compiled)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Compiled> {
        self.compiled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether `module` is installed and consumes `topic` at `version`.
    async fn consumes(&self, module: &str, topic: &str, version: u32) -> Result<(), EventsError> {
        let consumes = self
            .installed(module)
            .await?
            .and_then(|m| m.events)
            .is_some_and(|e| {
                e.consumes
                    .iter()
                    .any(|c| c.topic == topic && c.version == version)
            });
        consumes.then_some(()).ok_or(EventsError::NotDeclared)
    }

    /// The events of `topic` at `version` that `module` reads after
    /// `after` (without it, after its confirmed cursor, or from the oldest
    /// kept), at most [`MAX_READ`]. With none, waits until one is stored
    /// or `deadline` ends, and reads once more.
    pub async fn read(
        &self,
        module: &str,
        topic: &str,
        version: u32,
        after: Option<i64>,
        deadline: impl Future<Output = ()> + Send,
    ) -> Result<Batch, EventsError> {
        self.consumes(module, topic, version).await?;
        let consumer = consumer(module, topic, version);
        let after = match after {
            Some(after) => after.max(0),
            None => self.cursor(&consumer).await?,
        };
        // Subscribed before the first read, so a store in between wakes it.
        let mut woken = self.wake.subscribe();
        let mut events = self.after(topic, version, after).await?;
        if events.is_empty() {
            let wake = async {
                loop {
                    match woken.recv().await {
                        Ok(t) if !t.is_empty() && t != topic => {}
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => break,
                        Err(broadcast::error::RecvError::Closed) => std::future::pending().await,
                    }
                }
            };
            tokio::select! {
                () = wake => events = self.after(topic, version, after).await?,
                () = deadline => {}
            }
        }
        let cursor = events.last().map_or(after, |e| e.seq);
        Ok(Batch { events, cursor })
    }

    async fn after(
        &self,
        topic: &str,
        version: u32,
        after: i64,
    ) -> Result<Vec<StoredEvent>, EventsError> {
        self.store
            .after(topic, version, after, MAX_READ)
            .await
            .map_err(EventsError::Unavailable)
    }

    async fn cursor(&self, consumer: &Consumer) -> Result<i64, EventsError> {
        let cursor = self.store.cursor(consumer).await;
        Ok(cursor.map_err(EventsError::Unavailable)?.unwrap_or(0))
    }

    /// `module` handled `topic` at `version` up to `cursor`.
    pub async fn confirm(
        &self,
        module: &str,
        topic: &str,
        version: u32,
        cursor: i64,
    ) -> Result<(), EventsError> {
        self.consumes(module, topic, version).await?;
        self.store
            .confirm(&consumer(module, topic, version), cursor.max(0))
            .await
            .map_err(EventsError::Unavailable)
    }
}

fn consumer(module: &str, topic: &str, version: u32) -> Consumer {
    Consumer {
        module: module.into(),
        topic: topic.into(),
        version,
    }
}
