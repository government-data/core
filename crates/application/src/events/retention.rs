//! How long the module events are kept (quality-sdk decision 0017,
//! section 4): an event goes once it is [`RETENTION`] old and every module
//! that consumes its topic and version, installed or removed (paused), has
//! confirmed past it; with no consumer, once it is [`RETENTION`] old. A
//! purged module's cursors go first, so it holds nothing. A daily job of
//! the core runs it; running it twice changes nothing.

use std::time::Duration;

use chrono::{DateTime, Utc};

use super::service::{EventsError, EventsService};
use super::store::Consumer;

/// How long an event is kept at least.
pub const RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);

/// What one run deleted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Retained {
    /// Cursors of purged modules.
    pub cursors: u64,
    pub events: u64,
}

impl EventsService {
    /// Delete what is no longer kept at `now`.
    pub async fn retain(&self, now: DateTime<Utc>) -> Result<Retained, EventsError> {
        let unavailable = EventsError::Unavailable;
        let installed = self
            .updates
            .installed_modules()
            .await
            .map_err(unavailable)?;
        let kept: Vec<String> = self
            .updates
            .declared_modules()
            .await
            .map_err(unavailable)?
            .into_iter()
            .map(|d| d.name)
            .collect();
        // Before the agent ever told the list, nothing is known to be purged.
        let cursors = if kept.is_empty() {
            0
        } else {
            self.store
                .forget_cursors(&kept)
                .await
                .map_err(unavailable)?
        };
        let consumers: Vec<Consumer> = installed
            .into_iter()
            .flat_map(|m| {
                let consumes = m.events.map(|e| e.consumes).unwrap_or_default();
                consumes.into_iter().map(move |c| Consumer {
                    module: m.name.clone(),
                    topic: c.topic,
                    version: c.version,
                })
            })
            .collect();
        let age = chrono::Duration::from_std(RETENTION).unwrap_or_default();
        let events = self
            .store
            .delete_confirmed(&consumers, now - age)
            .await
            .map_err(unavailable)?;
        Ok(Retained { cursors, events })
    }
}
