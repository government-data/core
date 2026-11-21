//! What still runs, for the update agent (decision 0003). The platform runs
//! no work of its own: each installed module counts its own, and the
//! platform adds the answers without naming any module.

use std::time::Duration;

use serde::Serialize;

/// One module's work in flight.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModuleWork {
    /// Taken and not finished.
    pub running: u64,
    /// Waiting: it runs after the update.
    pub queued: u64,
    /// Due within the horizon asked for.
    pub scheduled_soon: u64,
}

/// Port: where the modules' work comes from.
#[async_trait::async_trait]
pub trait ModulesWork: Send + Sync {
    /// Each module's work, by module name; `Err` when it did not answer in
    /// time or its answer did not read.
    async fn work(&self, horizon: Duration) -> Vec<(String, Result<ModuleWork, String>)>;
}

/// What still runs: the agent waits for it to be drained to update.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DrainStatus {
    /// Work already taken, in every module.
    pub running: u64,
    /// Deployments being installed.
    pub deploying: u64,
    /// Queued work (it runs when maintenance ends).
    pub queued: u64,
    /// Work due within the requested horizon.
    pub scheduled_soon: u64,
    /// The modules that did not answer: they may be running something.
    pub not_answering: Vec<String>,
}

impl DrainStatus {
    /// Nothing half done: it can update without cutting anything. A module
    /// that did not answer counts as not drained.
    pub fn is_drained(&self) -> bool {
        self.running == 0 && self.deploying == 0 && self.not_answering.is_empty()
    }

    /// Add every module's answer.
    pub fn add(&mut self, answers: Vec<(String, Result<ModuleWork, String>)>) {
        for (module, answer) in answers {
            match answer {
                Ok(w) => {
                    self.running += w.running;
                    self.queued += w.queued;
                    self.scheduled_soon += w.scheduled_soon;
                }
                Err(e) => {
                    tracing::warn!(module, "drain: the module did not answer: {e}");
                    self.not_answering.push(module);
                }
            }
        }
    }
}
