//! What the updates use case stores and shows, and the port it stores it
//! through.

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::{DaemonReport, UpdatePolicy};

/// The installation in maintenance (or the last time it was).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Maintenance {
    pub reason: String,
    pub started_at: DateTime<Utc>,
    /// Until when, if nobody ends it before. Ending it sets it to now.
    pub until: DateTime<Utc>,
}

impl Maintenance {
    pub fn active_at(&self, now: DateTime<Utc>) -> bool {
        now < self.until
    }
}

/// Why maintenance could not be entered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaintenanceError {
    /// The request is not valid (no reason, a length out of range).
    Invalid(String),
    /// The database did not answer.
    Unavailable(String),
}

impl std::fmt::Display for MaintenanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(e) | Self::Unavailable(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for MaintenanceError {}

/// An "Update now".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateRequest {
    pub id: String,
    pub at: DateTime<Utc>,
    /// The email of whoever asked, if the account still exists.
    pub by: Option<String>,
}

/// How an attempt ended, for the history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HistoryEntry {
    pub kind: String,
    pub version: String,
    pub from_version: String,
    pub detail: String,
    pub at: DateTime<Utc>,
}

/// What is stored of the updates (one row).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpdaterRow {
    /// What the daemon reported last, and when.
    pub report: Option<(serde_json::Value, DateTime<Utc>)>,
    /// The policy, when and who (email) changed it.
    pub policy: Option<(serde_json::Value, DateTime<Utc>, Option<String>)>,
    pub request: Option<UpdateRequest>,
    /// The available version already announced by email.
    pub notified_available: Option<String>,
    /// The last outcome announced ([`super::Outcome::key`]).
    pub notified_result: Option<String>,
}

/// Port: where maintenance, what the daemon reports and what the
/// administrator decided are stored.
#[async_trait::async_trait]
pub trait UpdatesStore: Send + Sync {
    /// The current maintenance or the last one (active or not).
    async fn maintenance(&self) -> Result<Option<Maintenance>, String>;
    /// Enter (or stay in) maintenance until `until`. If it already was, it
    /// keeps when it started.
    async fn begin_maintenance(
        &self,
        reason: &str,
        until: DateTime<Utc>,
    ) -> Result<Maintenance, String>;
    /// Leave now (it does nothing if it was not in maintenance).
    async fn end_maintenance(&self) -> Result<(), String>;
    async fn updater(&self) -> Result<UpdaterRow, String>;
    async fn save_report(&self, report: &serde_json::Value) -> Result<(), String>;
    async fn save_policy(&self, policy: &serde_json::Value, admin_id: &str) -> Result<(), String>;
    /// A new "Update now" (it replaces the previous one).
    async fn new_request(&self, admin_id: &str) -> Result<UpdateRequest, String>;
    /// Write down what was announced (what comes as `None` does not change).
    async fn mark_notified(
        &self,
        available: Option<&str>,
        result: Option<&str>,
    ) -> Result<(), String>;
    /// Add to the history (a repeated one is ignored).
    async fn add_history(&self, entry: &HistoryEntry) -> Result<(), String>;
    /// The latest of the history, newest first.
    async fn history(&self, limit: u32) -> Result<Vec<HistoryEntry>, String>;
    /// The platform's time zone as saved in Settings (decision 0079), `None`
    /// when it never was.
    async fn platform_timezone(&self) -> Result<Option<String>, String>;
    /// Replace the installed modules (decision 0003).
    async fn save_modules(&self, modules: &[super::InstalledModule]) -> Result<(), String>;
    /// The installed modules as last saved; none when they never were.
    async fn modules(&self) -> Result<Vec<super::InstalledModule>, String>;
}

/// What an administrator sees in Settings → Updates.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UpdatesView {
    /// Whether the daemon ever reported (without it, the installation is
    /// updated by hand or predates the daemon).
    pub daemon: bool,
    /// When it last reported its state.
    pub reported_at: Option<DateTime<Utc>>,
    /// It has not reported anything for more than [`super::DAEMON_STALE`].
    pub stale: bool,
    pub report: DaemonReport,
    pub policy: UpdatePolicy,
    pub policy_updated_at: Option<DateTime<Utc>>,
    pub policy_updated_by: Option<String>,
    /// The "Update now" the daemon has not handled yet.
    pub pending_request: Option<UpdateRequest>,
    pub maintenance: Option<Maintenance>,
    pub history: Vec<HistoryEntry>,
}

/// Why something of the updates could not be changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdatesError {
    /// The field and the problem.
    Invalid(&'static str, String),
    /// There is nothing to install, or the daemon is not there.
    Conflict(String),
    Unavailable(String),
}

impl std::fmt::Display for UpdatesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(field, e) => write!(f, "{field}: {e}"),
            Self::Conflict(e) | Self::Unavailable(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for UpdatesError {}
