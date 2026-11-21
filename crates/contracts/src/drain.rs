//! What a module still has to finish before the installation updates
//! (decision 0003): the update agent waits for nothing to be half done.

use std::time::Duration;

/// A module's work in flight.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Work {
    /// Taken and not finished: an update would cut it.
    pub running: u64,
    /// Waiting to be taken: it runs after the update.
    pub queued: u64,
    /// Due to start within the horizon asked for.
    pub scheduled_soon: u64,
}

/// A module that counts its own work. The compositor asks every module
/// that has one and adds the answers; it never names the module.
#[async_trait::async_trait]
pub trait Drain: Send + Sync {
    /// The work in flight now, and what is due within `horizon`.
    async fn work(&self, horizon: Duration) -> Result<Work, String>;
}
