//! A module's events, as the update agent tells them in `PUT
//! /updater/v1/modules` (quality-sdk decision 0017): the topics it
//! publishes, each at a version with its JSON Schema, and the topics it
//! reads. A topic is `<module>.<name>`, the core's own `core.<name>`; a
//! module provides only topics that start with its name. Each schema is
//! compiled when the list is told ([`crate::events::EventSchemas`]): one
//! that does not compile refuses the list, and the list held is kept.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::events::EventSchemas;

/// The most topics a module provides, and the most it consumes.
pub(super) const MAX_EVENTS: usize = 32;
/// The largest schema, as JSON text.
pub(super) const MAX_SCHEMA_BYTES: usize = 64 * 1024;
/// The longest topic.
const MAX_TOPIC: usize = 127;

/// A module's events.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleEvents {
    #[serde(default)]
    pub provides: Vec<ProvidedEvent>,
    #[serde(default)]
    pub consumes: Vec<ConsumedEvent>,
}

/// A topic a module publishes, at a version, with its schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvidedEvent {
    pub topic: String,
    pub version: u32,
    pub schema: serde_json::Value,
}

/// A topic a module reads, at a version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsumedEvent {
    pub topic: String,
    pub version: u32,
}

/// Why a module's events are refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Refusal {
    TooMany,
    /// A topic that is not well formed, or not the module's to provide.
    BadTopic(String),
    BadVersion(String),
    Repeated(String),
    BigSchema(String),
    /// The schema does not compile, and why.
    BadSchema(String, String),
}

/// `<publisher>.<segment>[.<segment>…]`, each segment `[a-z0-9_]`; the
/// publisher's part.
fn publisher(topic: &str) -> Option<&str> {
    let (publisher, name) = topic.split_once('.')?;
    let segment = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    (topic.len() <= MAX_TOPIC && super::modules::is_name(publisher) && name.split('.').all(segment))
        .then_some(publisher)
}

/// The shape of `module`'s events, before any schema is compiled.
pub(super) fn check(module: &str, events: Option<&ModuleEvents>) -> Result<(), Refusal> {
    let Some(e) = events else { return Ok(()) };
    if e.provides.len() > MAX_EVENTS || e.consumes.len() > MAX_EVENTS {
        return Err(Refusal::TooMany);
    }
    let mut provided = BTreeSet::new();
    for p in &e.provides {
        if publisher(&p.topic) != Some(module) || module == "core" {
            return Err(Refusal::BadTopic(p.topic.clone()));
        }
        if p.version == 0 {
            return Err(Refusal::BadVersion(p.topic.clone()));
        }
        if !provided.insert((p.topic.as_str(), p.version)) {
            return Err(Refusal::Repeated(p.topic.clone()));
        }
        let size = serde_json::to_vec(&p.schema).map_or(usize::MAX, |b| b.len());
        if size > MAX_SCHEMA_BYTES || !p.schema.is_object() {
            return Err(Refusal::BigSchema(p.topic.clone()));
        }
    }
    let mut consumed = BTreeSet::new();
    for c in &e.consumes {
        if publisher(&c.topic).is_none() {
            return Err(Refusal::BadTopic(c.topic.clone()));
        }
        if c.version == 0 {
            return Err(Refusal::BadVersion(c.topic.clone()));
        }
        if !consumed.insert((c.topic.as_str(), c.version)) {
            return Err(Refusal::Repeated(c.topic.clone()));
        }
    }
    Ok(())
}

/// Compile every schema `module` provides.
pub(super) fn compile(
    schemas: &dyn EventSchemas,
    events: Option<&ModuleEvents>,
) -> Result<(), Refusal> {
    for p in events.map(|e| e.provides.as_slice()).unwrap_or_default() {
        schemas
            .compile(&p.schema)
            .map_err(|why| Refusal::BadSchema(p.topic.clone(), why))?;
    }
    Ok(())
}

impl super::UpdatesService {
    /// Compile every schema the modules provide; without a compiler, a
    /// module that provides a topic is refused (its events could not be
    /// validated).
    pub(super) fn compile_events(&self, modules: &[super::InstalledModule]) -> Result<(), String> {
        for m in modules {
            let provides = m.events.as_ref().is_some_and(|e| !e.provides.is_empty());
            let compiled = match &self.event_schemas {
                Some(schemas) => compile(schemas.as_ref(), m.events.as_ref()),
                None if provides => Err(Refusal::BadSchema(
                    m.events
                        .as_ref()
                        .map(|e| e.provides[0].topic.clone())
                        .unwrap_or_default(),
                    "no schema compiler".into(),
                )),
                None => Ok(()),
            };
            compiled.map_err(|refusal| refusal_text(&m.name, refusal))?;
        }
        Ok(())
    }
}

/// What the agent is answered when `module`'s events are refused.
pub(super) fn refusal_text(module: &str, refusal: Refusal) -> String {
    use Refusal as R;
    let name = module;
    match refusal {
        R::TooMany => texts::tr!("app.updates.modules.tooManyEvents", name = name),
        R::BadTopic(t) => texts::tr!("app.updates.modules.badTopic", name = name, topic = t),
        R::BadVersion(t) => {
            texts::tr!(
                "app.updates.modules.badEventVersion",
                name = name,
                topic = t
            )
        }
        R::Repeated(t) => texts::tr!("app.updates.modules.repeatedEvent", name = name, topic = t),
        R::BigSchema(t) => texts::tr!("app.updates.modules.bigSchema", name = name, topic = t),
        R::BadSchema(t, why) => texts::tr!(
            "app.updates.modules.badSchema",
            name = name,
            topic = t,
            why = why
        ),
    }
}

#[cfg(test)]
#[path = "module_events_tests.rs"]
mod tests;
