//! The platform's time zone from a command on the server (`server timezone`,
//! decision 0079): read and set over the store, outside a running service.
//! Setting it records who did it as the command (no person signed in) and
//! leaves a `ResourceChanged` in the audit.

use std::sync::Arc;
use std::time::SystemTime;

use domain::{AuditEventType, AuditRepository, NewActionAudit};

use super::platform::checked_timezone;
use super::{PlatformSettings, SettingsError, SettingsStore, PLATFORM};

/// What the audit says was changed.
const RESOURCE: &str = "settings/platform";

/// The platform's time zone for a command.
pub struct PlatformTimezone {
    store: Arc<dyn SettingsStore>,
    audit: Arc<dyn AuditRepository>,
}

impl std::fmt::Debug for PlatformTimezone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformTimezone").finish_non_exhaustive()
    }
}

impl PlatformTimezone {
    pub fn new(store: Arc<dyn SettingsStore>, audit: Arc<dyn AuditRepository>) -> Self {
        Self { store, audit }
    }

    /// The saved time zone, or UTC when it was never saved or does not
    /// parse (what the update window and new schedules use).
    pub async fn get(&self) -> Result<String, String> {
        let saved = self.store.load(PLATFORM).await?.and_then(|s| {
            serde_json::from_value::<PlatformSettings>(s.data)
                .ok()
                .map(|p| p.timezone)
        });
        Ok(schedule_core::timezone_or_utc(saved.as_deref()))
    }

    /// Validate `name` and save it as the platform's time zone; `command`
    /// is what the audit records as the user agent. Returns the saved name.
    pub async fn set(&self, name: &str, command: &str) -> Result<String, SettingsError> {
        let timezone = checked_timezone(name)?;
        let value = serde_json::to_value(PlatformSettings {
            timezone: timezone.clone(),
        })
        .map_err(|e| SettingsError::Internal(e.to_string()))?;
        // No person: `updated_by` stays empty.
        self.store
            .save(PLATFORM, value, "")
            .await
            .map_err(SettingsError::Internal)?;
        let event = NewActionAudit {
            event_type: AuditEventType::ResourceChanged,
            admin_id: None,
            service_account: None,
            job_id: None,
            action: iam::catalog::CONFIGURE.into(),
            resource: RESOURCE.into(),
            ip: "127.0.0.1".into(),
            user_agent: Some(command.into()),
            occurred_at: SystemTime::now(),
        };
        if let Err(e) = self.audit.record_action(&event).await {
            tracing::warn!("platform time zone: failed to record audit event: {e}");
        }
        tracing::info!(%timezone, command, "platform time zone set by a command");
        Ok(timezone)
    }
}

#[cfg(test)]
#[path = "platform_command_tests.rs"]
mod tests;
