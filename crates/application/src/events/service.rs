//! The module events' use case (quality-sdk decision 0017): a module
//! publishes only the topics and versions it provides, each event
//! validated against the schema it declares. What a module declares is read from the
//! installed modules as the update agent last told them, so a version
//! taken out of a module's `provides` stops being accepted at once.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::store::{EventsStore, NewEvent};
use super::{EventSchema, EventSchemas};
use crate::updates::{InstalledModule, UpdatesService};

/// The largest `data`, as JSON text.
pub const MAX_DATA_BYTES: usize = 256 * 1024;

/// Why an event is not taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventsError {
    /// The caller's module does not provide the topic at that version.
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

type Compiled = HashMap<(String, String, u32), (Value, Arc<dyn EventSchema>)>;

/// Publishes the modules' events.
#[derive(Clone)]
pub struct EventsService {
    store: Arc<dyn EventsStore>,
    updates: UpdatesService,
    schemas: Arc<dyn EventSchemas>,
    /// Each declared schema compiled once, by module, topic and version,
    /// with the schema it was compiled from.
    compiled: Arc<Mutex<Compiled>>,
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
        }
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
}
