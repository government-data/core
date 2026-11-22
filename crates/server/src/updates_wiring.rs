//! Connects the updates ([`UpdatesService`]) with Postgres. It lives in the
//! composition root because `infrastructure` does not know `application`.
//!
//! [`UpdatesService`]: application::UpdatesService

use application::{HistoryEntry, Maintenance, UpdateRequest, UpdaterRow, UpdatesStore};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use infrastructure::{HistoryRecord, MaintenanceRow, PgMaintenanceStore};

/// [`UpdatesStore`] on Postgres.
pub(crate) struct Store(pub(crate) PgMaintenanceStore);

fn maintenance((reason, started_at, until): MaintenanceRow) -> Maintenance {
    Maintenance {
        reason,
        started_at,
        until,
    }
}

fn request((id, at, by): (String, DateTime<Utc>, Option<String>)) -> UpdateRequest {
    UpdateRequest { id, at, by }
}

#[async_trait]
impl UpdatesStore for Store {
    async fn maintenance(&self) -> Result<Option<Maintenance>, String> {
        Ok(self.0.maintenance().await?.map(maintenance))
    }

    async fn begin_maintenance(
        &self,
        reason: &str,
        until: DateTime<Utc>,
    ) -> Result<Maintenance, String> {
        self.0.begin(reason, until).await.map(maintenance)
    }

    async fn end_maintenance(&self) -> Result<(), String> {
        self.0.end().await
    }

    async fn updater(&self) -> Result<UpdaterRow, String> {
        let r = self.0.updater().await?;
        Ok(UpdaterRow {
            report: r.report,
            policy: r.policy,
            request: r.request.map(request),
            notified_available: r.notified_available,
            notified_result: r.notified_result,
        })
    }

    async fn save_report(&self, report: &serde_json::Value) -> Result<(), String> {
        self.0.save_report(report).await
    }

    async fn platform_timezone(&self) -> Result<Option<String>, String> {
        self.0.platform_timezone().await
    }

    async fn save_policy(&self, policy: &serde_json::Value, admin_id: &str) -> Result<(), String> {
        self.0.save_policy(policy, admin_id).await
    }

    async fn new_request(&self, admin_id: &str) -> Result<UpdateRequest, String> {
        self.0.new_request(admin_id).await.map(request)
    }

    async fn mark_notified(
        &self,
        available: Option<&str>,
        result: Option<&str>,
    ) -> Result<(), String> {
        self.0.mark_notified(available, result).await
    }

    async fn add_history(&self, e: &HistoryEntry) -> Result<(), String> {
        self.0
            .add_history(&HistoryRecord {
                kind: e.kind.clone(),
                version: e.version.clone(),
                from_version: e.from_version.clone(),
                detail: e.detail.clone(),
                at: e.at,
            })
            .await
    }

    async fn history(&self, limit: u32) -> Result<Vec<HistoryEntry>, String> {
        Ok(self
            .0
            .history(limit)
            .await?
            .into_iter()
            .map(|h| HistoryEntry {
                kind: h.kind,
                version: h.version,
                from_version: h.from_version,
                detail: h.detail,
                at: h.at,
            })
            .collect())
    }
}
