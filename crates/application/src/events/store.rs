//! What the module events are stored through (quality-sdk decision 0017):
//! the events in order, once per id.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;

/// An event as its publisher sends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvent {
    /// The id from the publisher's outbox (a UUID, canonical form).
    pub id: String,
    pub topic: String,
    pub version: u32,
    pub occurred_at: DateTime<Utc>,
    /// The module's, never interpreted or logged by the core.
    pub data: Value,
}

/// The port the events are stored through.
#[async_trait]
pub trait EventsStore: Send + Sync {
    /// Store `event` from `publisher` and wake whoever waits for its
    /// topic once it is committed: `true` when it is new, `false` when an
    /// event with its id was already stored (nothing changes).
    async fn store(&self, publisher: &str, event: &NewEvent) -> Result<bool, String>;
}
