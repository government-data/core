//! The installation's updates.
//!
//! The host daemon (`orquestador-actualizador`) updates, never the API: the
//! API only tells it whether it can (what still runs) and, meanwhile, goes
//! into **maintenance**: no new work is handed out and what already runs
//! finishes. What is queued meanwhile (a schedule, someone pressing "Run")
//! waits in the queue and runs once on the way out.
//!
//! Maintenance lives in the database, not in the API's memory: every replica
//! sees it and it survives the update's own restart. It expires by itself
//! (`until`), so a daemon that dies halfway does not leave the platform
//! frozen.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;

use crate::admins::AdminContacts;
use crate::settings::MailSlot;

mod drain;
mod modules;
mod policy;
mod report;
mod store;
#[cfg(test)]
pub(crate) mod tests;

pub use drain::{DrainStatus, ModuleWork, ModulesWork};
pub use modules::InstalledModule;
pub use policy::UpdatePolicy;
pub use report::{DaemonReport, Outcome};
pub use store::{
    HistoryEntry, Maintenance, MaintenanceError, UpdateRequest, UpdaterRow, UpdatesError,
    UpdatesStore, UpdatesView,
};

/// If the daemon reports nothing in this time, the page warns that it does
/// not answer (it reports its state at least every 5 min).
pub const DAEMON_STALE: Duration = Duration::from_secs(15 * 60);

/// The longest that can be asked for at once (it is renewed if needed).
pub const MAX_MAINTENANCE: Duration = Duration::from_secs(6 * 3600);

/// The use case: enter and leave maintenance, see what is missing to update,
/// and handle what the daemon reports.
#[derive(Clone)]
pub struct UpdatesService {
    store: Arc<dyn UpdatesStore>,
    /// The Settings mail, and whom to tell.
    notices: Option<(MailSlot, Arc<dyn AdminContacts>)>,
    /// Where the modules' work in flight comes from.
    modules: Vec<Arc<dyn ModulesWork>>,
}

impl std::fmt::Debug for UpdatesService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdatesService").finish_non_exhaustive()
    }
}

impl UpdatesService {
    pub fn new(store: Arc<dyn UpdatesStore>) -> Self {
        Self {
            store,
            notices: None,
            modules: Vec::new(),
        }
    }

    /// Email the administrators about each new version and how each attempt
    /// ended.
    pub fn with_notices(mut self, mail: MailSlot, admins: Arc<dyn AdminContacts>) -> Self {
        self.notices = Some((mail, admins));
        self
    }

    /// Count the work in flight of the modules `work` answers for too.
    pub fn with_modules_work(mut self, work: Arc<dyn ModulesWork>) -> Self {
        self.modules.push(work);
        self
    }

    /// The maintenance, if it is active now.
    pub async fn active(&self) -> Result<Option<Maintenance>, String> {
        let now = Utc::now();
        Ok(self.store.maintenance().await?.filter(|m| m.active_at(now)))
    }

    /// Whether it is or was in maintenance within the last `grace`: after an
    /// update, runners take a few seconds to come back, and their silence is
    /// not an outage.
    pub async fn within(&self, grace: Duration) -> Result<bool, String> {
        let since = Utc::now() - chrono::Duration::from_std(grace).unwrap_or_default();
        Ok(self
            .store
            .maintenance()
            .await?
            .is_some_and(|m| m.until > since))
    }

    pub async fn begin(
        &self,
        reason: &str,
        length: Duration,
    ) -> Result<Maintenance, MaintenanceError> {
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(MaintenanceError::Invalid(texts::tr!(
                "app.updates.maintenanceReasonMissing"
            )));
        }
        if length.is_zero() || length > MAX_MAINTENANCE {
            return Err(MaintenanceError::Invalid(texts::tr!(
                "app.updates.maintenanceDuration",
                as_secs = MAX_MAINTENANCE.as_secs() / 3600
            )));
        }
        let reason: String = reason.chars().take(200).collect();
        let until = Utc::now() + chrono::Duration::from_std(length).unwrap_or_default();
        let maintenance = self
            .store
            .begin_maintenance(&reason, until)
            .await
            .map_err(MaintenanceError::Unavailable)?;
        tracing::warn!(reason = %maintenance.reason, until = %maintenance.until, "maintenance: no new work is handed out");
        Ok(maintenance)
    }

    pub async fn end(&self) -> Result<(), String> {
        self.store.end_maintenance().await?;
        tracing::info!("maintenance over: work is handed out again");
        Ok(())
    }

    /// What every module still runs, and what is due within `horizon`.
    pub async fn drain_status(&self, horizon: Duration) -> DrainStatus {
        let mut status = DrainStatus::default();
        for modules in &self.modules {
            status.add(modules.work(horizon).await);
        }
        status
    }

    fn policy_of(row: &UpdaterRow) -> UpdatePolicy {
        row.policy
            .as_ref()
            .and_then(|(p, _, _)| serde_json::from_value(p.clone()).ok())
            .unwrap_or_default()
    }

    fn report_of(row: &UpdaterRow) -> DaemonReport {
        row.report
            .as_ref()
            .and_then(|(r, _)| serde_json::from_value(r.clone()).ok())
            .unwrap_or_default()
    }

    /// The request the daemon has not handled yet.
    fn pending(row: &UpdaterRow, report: &DaemonReport) -> Option<UpdateRequest> {
        row.request
            .clone()
            .filter(|r| report.handled_request.as_deref() != Some(r.id.as_str()))
    }

    /// What the daemon reads: the policy and the pending "Update now". The
    /// window's time zone is the platform's (decision 0079), UTC when it was
    /// never saved or does not parse; the policy's own is not read.
    pub async fn orders(&self) -> Result<serde_json::Value, String> {
        let row = self.store.updater().await?;
        let report = Self::report_of(&row);
        let request = Self::pending(&row, &report).map(|r| serde_json::json!({ "id": r.id }));
        let policy = self.in_platform_zone(Self::policy_of(&row)).await?;
        Ok(serde_json::json!({
            "policy": policy,
            "request": request,
        }))
    }

    /// The platform's time zone (decision 0079), UTC when it was never
    /// saved or does not parse.
    pub async fn platform_timezone(&self) -> Result<String, String> {
        let saved = self.store.platform_timezone().await?;
        Ok(schedule_core::timezone_or_utc(saved.as_deref()))
    }

    /// `policy` with its window in the platform's time zone: what the
    /// daemon follows, the page shows and the emails say. The stored
    /// window's own zone is not read (nor rewritten).
    async fn in_platform_zone(&self, mut policy: UpdatePolicy) -> Result<UpdatePolicy, String> {
        policy.window.timezone = self.platform_timezone().await?;
        Ok(policy)
    }

    /// What the daemon reports: stored as is for the page, the outcome is
    /// added to the history and what is new is announced.
    pub async fn report(&self, report: serde_json::Value) -> Result<(), UpdatesError> {
        if !report.is_object() {
            return Err(UpdatesError::Invalid(
                "report",
                texts::tr!("app.updates.objectExpected"),
            ));
        }
        let parsed: DaemonReport = serde_json::from_value(report.clone())
            .map_err(|e| UpdatesError::Invalid("report", e.to_string()))?;
        let unavailable = UpdatesError::Unavailable;
        let row = self.store.updater().await.map_err(unavailable)?;
        self.store.save_report(&report).await.map_err(unavailable)?;
        if let Some(last) = &parsed.last {
            self.store
                .add_history(&HistoryEntry {
                    kind: last.kind.clone(),
                    version: last.version.clone(),
                    from_version: last.from.clone(),
                    detail: last.detail.chars().take(4000).collect(),
                    at: last.at,
                })
                .await
                .map_err(unavailable)?;
        }
        self.notify(&row, &parsed).await;
        Ok(())
    }

    /// Announce what was not announced yet. What could not be sent is retried
    /// on the next report; without mail configured or without administrators
    /// with an email it counts as announced (it is not sent late).
    async fn notify(&self, row: &UpdaterRow, report: &DaemonReport) {
        let Some((mail, admins)) = &self.notices else {
            return;
        };
        let new_available = report
            .available
            .as_ref()
            .filter(|a| !a.version.is_empty())
            .filter(|a| row.notified_available.as_deref() != Some(a.version.as_str()));
        let new_result = report
            .last
            .as_ref()
            .filter(|o| row.notified_result.as_deref() != Some(o.key().as_str()));
        if new_available.is_none() && new_result.is_none() {
            return;
        }
        let to = match admins.admin_emails().await {
            Ok(to) => to,
            Err(e) => {
                tracing::warn!("updates: could not read whom to notify: {e}");
                return;
            }
        };
        let can_send = mail.configured() && !to.is_empty();
        if let Some(available) = new_available {
            let sent = if can_send {
                let policy = Self::policy_of(row);
                let policy = self
                    .in_platform_zone(policy.clone())
                    .await
                    .unwrap_or(policy);
                let m = available.mail(&report.installed, &policy, to.clone());
                mail.send(&m).await
            } else {
                Ok(())
            };
            match sent {
                Ok(()) => {
                    let _ = self
                        .store
                        .mark_notified(Some(&available.version), None)
                        .await;
                }
                Err(e) => {
                    tracing::warn!("updates: could not announce the new version: {e}")
                }
            }
        }
        if let Some(outcome) = new_result {
            let sent = match outcome.mail(to) {
                Some(m) if can_send => mail.send(&m).await,
                _ => Ok(()),
            };
            match sent {
                Ok(()) => {
                    let _ = self.store.mark_notified(None, Some(&outcome.key())).await;
                }
                Err(e) => tracing::warn!("updates: could not announce the outcome: {e}"),
            }
        }
    }

    /// What the page shows.
    pub async fn view(&self) -> Result<UpdatesView, String> {
        let row = self.store.updater().await?;
        let report = Self::report_of(&row);
        let reported_at = row.report.as_ref().map(|(_, at)| *at);
        let stale = reported_at
            .is_some_and(|at| (Utc::now() - at).to_std().unwrap_or_default() > DAEMON_STALE);
        Ok(UpdatesView {
            daemon: reported_at.is_some(),
            reported_at,
            stale,
            pending_request: Self::pending(&row, &report),
            report,
            policy: self.in_platform_zone(Self::policy_of(&row)).await?,
            policy_updated_at: row.policy.as_ref().map(|(_, at, _)| *at),
            policy_updated_by: row.policy.as_ref().and_then(|(_, _, by)| by.clone()),
            maintenance: self.active().await?,
            history: self.store.history(20).await?,
        })
    }

    pub async fn set_policy(
        &self,
        admin_id: &str,
        policy: UpdatePolicy,
    ) -> Result<(), UpdatesError> {
        policy
            .validate()
            .map_err(|(field, e)| UpdatesError::Invalid(field, e))?;
        let value =
            serde_json::to_value(&policy).map_err(|e| UpdatesError::Unavailable(e.to_string()))?;
        self.store
            .save_policy(&value, admin_id)
            .await
            .map_err(UpdatesError::Unavailable)?;
        tracing::info!(mode = ?policy.mode, paused = policy.paused, "update policy changed");
        Ok(())
    }

    /// "Update now": the daemon takes it on its next round.
    pub async fn request_now(&self, admin_id: &str) -> Result<UpdateRequest, UpdatesError> {
        let row = self
            .store
            .updater()
            .await
            .map_err(UpdatesError::Unavailable)?;
        let report = Self::report_of(&row);
        if row.report.is_none() {
            return Err(UpdatesError::Conflict(texts::tr!(
                "app.updates.noUpdateService"
            )));
        }
        if report.phase != "idle" && report.phase != "prepared" {
            return Err(UpdatesError::Conflict(texts::tr!(
                "app.updates.alreadyInProgress",
                phase = report.phase
            )));
        }
        let request = self
            .store
            .new_request(admin_id)
            .await
            .map_err(UpdatesError::Unavailable)?;
        tracing::info!(request = %request.id, "update now");
        Ok(request)
    }
}
