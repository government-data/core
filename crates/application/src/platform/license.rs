//! The installation's license and updates.

use iam::catalog::{MANAGE_LICENSE, MANAGE_UPDATES};
use iam::Actor;

use super::{Edition, PlatformError, PlatformService, R};

/// The first day of the current month (the monthly quotas count from there).
fn month_start() -> chrono::DateTime<chrono::Utc> {
    use chrono::{Datelike, TimeZone, Utc};
    let now = Utc::now();
    Utc.with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
        .single()
        .unwrap_or(now)
}

fn updates_error(e: crate::UpdatesError) -> PlatformError {
    match e {
        crate::UpdatesError::Invalid(field, e) => PlatformError::Invalid(format!("{field}: {e}")),
        crate::UpdatesError::Conflict(e) => PlatformError::Conflict(e),
        crate::UpdatesError::Unavailable(e) => PlatformError::Internal(e),
    }
}

impl PlatformService {
    /// The license and what is used of it (administrators only).
    pub async fn license(
        &self,
        actor: &Actor,
    ) -> R<Option<(crate::LicenseView, license_model::Usage)>> {
        Self::require(actor, MANAGE_LICENSE)?;
        let Some(license) = &self.license else {
            return Ok(None);
        };
        Ok(Some((
            license.view(),
            self.repo.usage(month_start()).await?,
        )))
    }

    fn license_service(&self) -> R<&crate::LicenseService> {
        self.license
            .as_ref()
            .ok_or_else(|| PlatformError::Invalid(texts::tr!("app.platform.noLicenseUsed")))
    }

    /// Change the activation key and activate it with the cloud.
    pub async fn set_license_key(&self, actor: &Actor, key: &str) -> R<()> {
        Self::require(actor, MANAGE_LICENSE)?;
        self.license_service()?
            .set_key(key)
            .await
            .map_err(PlatformError::Invalid)
    }

    /// Renew with the cloud now (without waiting for the next day).
    pub async fn refresh_license(&self, actor: &Actor) -> R<()> {
        Self::require(actor, MANAGE_LICENSE)?;
        self.license_service()?
            .refresh()
            .await
            .map_err(PlatformError::Invalid)
    }

    /// The license's payments and renewal (admin only).
    pub async fn license_billing(&self, actor: &Actor) -> R<crate::BillingView> {
        Self::require(actor, MANAGE_LICENSE)?;
        self.license_service()?
            .billing()
            .await
            .map_err(PlatformError::Invalid)
    }

    /// Where the contract stands, to warn in the UI (nothing if it is current).
    pub fn license_notice(&self) -> Option<license_model::ContractStatus> {
        self.license
            .as_ref()
            .and_then(|l| l.contract())
            .filter(|c| *c != license_model::ContractStatus::Current)
    }

    /// Why the installation is read-only, if it is.
    pub fn read_only(&self) -> Option<String> {
        self.license.as_ref().and_then(|l| l.read_only())
    }

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
