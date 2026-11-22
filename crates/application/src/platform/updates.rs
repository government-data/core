//! The installation's updates.

use iam::catalog::MANAGE_UPDATES;
use iam::Actor;

use super::{Edition, PlatformError, PlatformService, R};

fn updates_error(e: crate::UpdatesError) -> PlatformError {
    match e {
        crate::UpdatesError::Invalid(field, e) => PlatformError::Invalid(format!("{field}: {e}")),
        crate::UpdatesError::Conflict(e) => PlatformError::Conflict(e),
        crate::UpdatesError::Unavailable(e) => PlatformError::Internal(e),
    }
}

impl PlatformService {
    /// The installation's updates (administrators only). `None` without the
    /// service (we update the online edition ourselves).
    pub async fn updates(&self, actor: &Actor) -> R<Option<crate::UpdatesView>> {
        Self::require(actor, MANAGE_UPDATES)?;
        let Some(updates) = &self.updates else {
            return Ok(None);
        };
        if self.edition != Edition::OnPremise {
            return Ok(None);
        }
        updates
            .view()
            .await
            .map(Some)
            .map_err(PlatformError::Internal)
    }

    pub async fn set_update_policy(&self, actor: &Actor, policy: crate::UpdatePolicy) -> R<()> {
        Self::require(actor, MANAGE_UPDATES)?;
        let updates = self.updates_on_premise()?;
        updates
            .set_policy(&actor.admin_id, policy)
            .await
            .map_err(updates_error)
    }

    /// "Update now": the host daemon takes it on its next round.
    pub async fn request_update(&self, actor: &Actor) -> R<crate::UpdateRequest> {
        Self::require(actor, MANAGE_UPDATES)?;
        let updates = self.updates_on_premise()?;
        updates
            .request_now(&actor.admin_id)
            .await
            .map_err(updates_error)
    }

    /// The platform's time zone, for everyone signed in (decision 0079):
    /// UTC when it was never saved, or without the updates' store or when
    /// it cannot be read.
    pub async fn platform_timezone(&self) -> String {
        let Some(updates) = &self.updates else {
            return schedule_core::timezone_or_utc(None);
        };
        updates.platform_timezone().await.unwrap_or_else(|e| {
            tracing::warn!("the platform's time zone: {e}");
            schedule_core::timezone_or_utc(None)
        })
    }

    /// The updates notice on top of every view: everyone sees the maintenance
    /// (what is run now runs when it ends); administrators, a new version, one
    /// that rolled back, or a critical state.
    pub async fn update_notice(&self, actor: &Actor) -> Option<serde_json::Value> {
        let updates = self.updates.as_ref()?;
        if let Ok(Some(m)) = updates.active().await {
            return Some(serde_json::json!({ "kind": "maintenance", "reason": m.reason }));
        }
        if !actor.may(MANAGE_UPDATES) || self.edition != Edition::OnPremise {
            return None;
        }
        let view = updates.view().await.ok()?;
        let r = &view.report;
        if r.phase == "critical" {
            return Some(serde_json::json!({ "kind": "critical" }));
        }
        let recent =
            |o: &crate::updates::Outcome| (chrono::Utc::now() - o.at) < chrono::Duration::days(3);
        if let Some(last) = r.last.as_ref().filter(|o| recent(o)) {
            if matches!(last.kind.as_str(), "rolled_back" | "restored") {
                return Some(serde_json::json!({
                    "kind": "rolled_back", "version": last.version, "from": last.from,
                }));
            }
        }
        let available = r.available.as_ref().filter(|a| !a.version.is_empty())?;
        (r.phase == "prepared" || r.phase == "idle").then(|| {
            serde_json::json!({
                "kind": "available",
                "version": available.version,
                "mode": view.policy.mode,
                "failed": r.failed.contains(&available.version),
            })
        })
    }

    fn updates_on_premise(&self) -> R<&crate::UpdatesService> {
        match &self.updates {
            Some(u) if self.edition == Edition::OnPremise => Ok(u),
            _ => Err(PlatformError::Conflict(texts::tr!(
                "app.platform.notUpdatedHere"
            ))),
        }
    }
}
