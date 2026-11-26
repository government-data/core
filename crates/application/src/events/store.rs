//! What the module events are stored through (quality-sdk decision 0017):
//! the events in order, once per id, and each consuming module's cursor.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;
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

/// An event as stored, with its place in the order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredEvent {
    pub seq: i64,
    pub id: String,
    pub topic: String,
    pub version: u32,
    pub publisher: String,
    pub occurred_at: DateTime<Utc>,
    pub data: Value,
}

/// A consuming module of a topic at a version.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Consumer {
    pub module: String,
    pub topic: String,
    pub version: u32,
}

/// The port the events are stored through.
#[async_trait]
pub trait EventsStore: Send + Sync {
    /// Store `event` from `publisher` and wake whoever waits for its
    /// topic once it is committed: `true` when it is new, `false` when an
    /// event with its id was already stored (nothing changes).
    async fn store(&self, publisher: &str, event: &NewEvent) -> Result<bool, String>;

    /// The events of `topic` at `version` after `after`, in order, at most
    /// `limit`.
    async fn after(
        &self,
        topic: &str,
        version: u32,
        after: i64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, String>;

    /// How far `consumer` confirmed, `None` when it never did.
    async fn cursor(&self, consumer: &Consumer) -> Result<Option<i64>, String>;

    /// `consumer` handled up to `cursor`; a cursor never goes back.
    async fn confirm(&self, consumer: &Consumer, cursor: i64) -> Result<(), String>;
}
