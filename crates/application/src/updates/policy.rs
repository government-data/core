//! When the installation updates: an administrator chooses it and the host
//! daemon reads it (`GET /internal/orders`). The names and the defaults are
//! the ones the daemon understands (the updater's `policy`).

use chrono::NaiveTime;
use serde::{Deserialize, Serialize};

/// How it updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UpdateMode {
    /// It only tells; it is updated with `orquestador actualizar` on the server.
    Manual,
    /// It tells and waits for an administrator to press "Update".
    #[default]
    Approval,
    /// By itself, inside the window.
    Automatic,
}

/// The automatic update's window, in the installation's time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateWindow {
    /// 1 = Monday … 7 = Sunday.
    pub days: Vec<u8>,
    /// "HH:MM". If `start` is after `end`, it crosses midnight.
    pub start: String,
    pub end: String,
    pub timezone: String,
}

impl Default for UpdateWindow {
    fn default() -> Self {
        Self {
            days: (1..=7).collect(),
            start: "02:00".into(),
            end: "05:00".into(),
            timezone: schedule_core::DEFAULT_TIMEZONE.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdatePolicy {
    pub mode: UpdateMode,
    pub window: UpdateWindow,
    /// How long to wait for what runs to finish before postponing.
    pub max_drain_secs: u64,
    /// In automatic mode, do not start if a schedule fires within this time
    /// (0: do not look).
    pub skip_if_scheduled_within_secs: u64,
    /// Paused: it does not update by itself (the button still works).
    pub paused: bool,
}

impl Default for UpdatePolicy {
    fn default() -> Self {
        Self {
            mode: UpdateMode::default(),
            window: UpdateWindow::default(),
            max_drain_secs: 30 * 60,
            skip_if_scheduled_within_secs: 15 * 60,
            paused: false,
        }
    }
}

impl UpdatePolicy {
    /// That the daemon can follow it. Returns the field and the problem.
    pub fn validate(&self) -> Result<(), (&'static str, String)> {
        let w = &self.window;
        let hour = |s: &str| NaiveTime::parse_from_str(s.trim(), "%H:%M").ok();
        let (Some(start), Some(end)) = (hour(&w.start), hour(&w.end)) else {
            return Err(("window", texts::tr!("app.updates.policy.timeFormat")));
        };
        if start == end {
            return Err(("window", texts::tr!("app.updates.policy.emptyWindow")));
        }
        if w.days.is_empty() || w.days.iter().any(|d| !(1..=7).contains(d)) {
            return Err(("window", texts::tr!("app.updates.policy.pickADay")));
        }
        if w.timezone.parse::<chrono_tz::Tz>().is_err() {
            return Err((
                "window",
                format!("zona horaria desconocida: {}", w.timezone),
            ));
        }
        if !(5 * 60..=4 * 3600).contains(&self.max_drain_secs) {
            return Err(("max_drain_secs", texts::tr!("app.updates.policy.drainWait")));
        }
        if self.skip_if_scheduled_within_secs > 2 * 3600 {
            return Err(("skip_if_scheduled_within_secs", "a lo sumo 2 h".into()));
        }
        Ok(())
    }

    /// In one line, for the emails.
    pub fn describe(&self) -> String {
        match self.mode {
            UpdateMode::Manual => texts::tr!("app.updates.policy.updatedByHand"),
            UpdateMode::Approval => texts::tr!("app.updates.policy.installFromSettings"),
            UpdateMode::Automatic if self.paused => {
                texts::tr!("app.updates.policy.automaticPaused")
            }
            UpdateMode::Automatic => texts::tr!(
                "app.updates.policy.installsInWindow",
                start = self.window.start,
                end = self.window.end,
                timezone = self.window.timezone
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_waits_for_approval_and_is_valid() {
        let p = UpdatePolicy::default();
        assert_eq!(p.mode, UpdateMode::Approval);
        assert_eq!(p.validate(), Ok(()));
        // What does not come is filled with the defaults.
        let p: UpdatePolicy =
            serde_json::from_value(serde_json::json!({ "mode": "automatic" })).unwrap();
        assert_eq!(p.max_drain_secs, 30 * 60);
        assert_eq!(p.window.start, "02:00");
    }

    #[test]
    fn a_window_the_daemon_cannot_follow_is_rejected() {
        let mut p = UpdatePolicy::default();
        p.window.start = "2am".into();
        assert_eq!(p.validate().unwrap_err().0, "window");
        let mut p = UpdatePolicy::default();
        p.window.end = p.window.start.clone();
        assert_eq!(p.validate().unwrap_err().0, "window");
        let mut p = UpdatePolicy::default();
        p.window.days = vec![];
        assert_eq!(p.validate().unwrap_err().0, "window");
        let mut p = UpdatePolicy::default();
        p.window.days = vec![8];
        assert_eq!(p.validate().unwrap_err().0, "window");
        let mut p = UpdatePolicy::default();
        p.window.timezone = "Mars/Olympus".into();
        assert_eq!(p.validate().unwrap_err().0, "window");
        let p = UpdatePolicy {
            max_drain_secs: 60,
            ..UpdatePolicy::default()
        };
        assert_eq!(p.validate().unwrap_err().0, "max_drain_secs");
        // Crossing midnight is fine.
        let mut p = UpdatePolicy::default();
        p.window.start = "22:00".into();
        p.window.end = "02:00".into();
        assert_eq!(p.validate(), Ok(()));
    }
}
