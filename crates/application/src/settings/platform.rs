//! The platform's section (decision 0079): the installation's time zone.

use std::str::FromStr;

use super::{invalid, PlatformSettings, Section, SettingsError, SettingsService, PLATFORM};

impl SettingsService {
    pub fn platform(&self) -> Section<PlatformSettings> {
        self.read().platform.clone()
    }

    /// Validate and save it; it holds nothing to apply, who needs the time
    /// zone reads it when it needs it.
    pub async fn save_platform(
        &self,
        mut s: PlatformSettings,
        actor: &str,
    ) -> Result<Section<PlatformSettings>, SettingsError> {
        s.timezone = s.timezone.trim().to_string();
        if chrono_tz::Tz::from_str(&s.timezone).is_err() {
            return Err(invalid(
                "timezone",
                &texts::tr!("app.settings.unknownTimezone"),
            ));
        }
        let source = self.persist(PLATFORM, &s, actor).await?;
        let saved = Section { value: s, source };
        self.current.update(|c| c.platform = saved.clone());
        Ok(saved)
    }
}
