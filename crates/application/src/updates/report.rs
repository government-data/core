//! What the daemon reports of its state (`POST /internal/report`, the
//! updater's `State`) and the emails that triggers: a new version (once per
//! version) and how each attempt ended (once per outcome).
//!
//! It is read leniently: a daemon newer or older than the API may bring more
//! or fewer fields, and what is not understood is ignored.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::policy::UpdatePolicy;
use crate::settings::{MailNotice, MailTone, OutgoingMail};

/// A version the cloud offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Available {
    pub version: String,
    pub notes: String,
}

/// How an attempt ended (the daemon's `last`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    /// updated, rolled_back, restored, critical, postponed, failed
    pub kind: String,
    pub version: String,
    pub from: String,
    pub detail: String,
    pub at: DateTime<Utc>,
}

/// The version it is going to (the daemon's `target`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Target {
    pub version: String,
}

/// The daemon's state, what the API uses of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DaemonReport {
    /// idle, prepared, draining, backing_up, applying, verifying,
    /// rolling_back, restoring, critical
    pub phase: String,
    #[serde(rename(deserialize = "installed_version"))]
    pub installed: String,
    pub available: Option<Available>,
    pub target: Option<Target>,
    pub failed: Vec<String>,
    pub last: Option<Outcome>,
    pub handled_request: Option<String>,
    /// What the cloud said last if it was not a version.
    pub warning: Option<String>,
    /// Since when it is in the current phase.
    pub phase_since: Option<DateTime<Utc>>,
    /// What happens inside the phase ("2 flujos corriendo").
    pub progress: Option<String>,
}

impl Outcome {
    /// To send each outcome's email only once.
    pub fn key(&self) -> String {
        format!("{}:{}:{}", self.kind, self.version, self.at.timestamp())
    }

    /// This outcome's email (a postponement is not told: it is retried by
    /// itself and shown on the page).
    pub fn mail(&self, to: Vec<String>) -> Option<OutgoingMail> {
        let (subject, intro, tone) = match self.kind.as_str() {
            "updated" => (
                texts::tr!("app.updates.report.updatedTo", version = self.version),
                texts::tr!("app.updates.report.succeeded"),
                MailTone::Success,
            ),
            "rolled_back" => (
                texts::tr!(
                    "app.updates.report.rolledBack",
                    version = self.version,
                    from = self.from
                ),
                texts::tr!("app.updates.report.rolledBackDetail"),
                MailTone::Warning,
            ),
            "restored" => (
                texts::tr!("app.updates.report.restored", version = self.version),
                texts::tr!("app.updates.report.restoredDetail"),
                MailTone::Warning,
            ),
            "critical" => (
                texts::tr!("app.updates.report.urgentDidNotComeBack"),
                texts::tr!("app.updates.report.failedCritical"),
                MailTone::Danger,
            ),
            "failed" => (
                texts::tr!("app.updates.report.notPrepared", version = self.version),
                texts::tr!("app.updates.report.nothingTouched"),
                MailTone::Warning,
            ),
            _ => return None,
        };
        Some(
            MailNotice::new(tone, subject.clone())
                .badge(texts::tr!("app.updates.report.label"))
                .prose(intro)
                .facts([
                    (texts::tr!("app.mail.version"), self.version.clone()),
                    (texts::tr!("app.mail.previousVersion"), self.from.clone()),
                ])
                .code(texts::tr!("app.mail.details"), self.detail.clone())
                .mail(to, subject),
        )
    }
}

impl Available {
    pub fn mail(&self, installed: &str, policy: &UpdatePolicy, to: Vec<String>) -> OutgoingMail {
        let subject = texts::tr!("app.updates.report.newVersion", version = self.version);
        let mut notice = MailNotice::new(MailTone::Info, subject.clone())
            .badge(texts::tr!("app.updates.report.label"))
            .prose(texts::tr!(
                "app.updates.report.installedAndReady",
                installed = installed,
                version = self.version
            ))
            .facts([
                (
                    texts::tr!("app.mail.installedVersion"),
                    installed.to_string(),
                ),
                (texts::tr!("app.mail.newVersion"), self.version.clone()),
            ]);
        if !self.notes.trim().is_empty() {
            notice = notice
                .heading(texts::tr!("app.updates.report.whatsNew"))
                .prose(self.notes.trim());
        }
        notice
            .callout(policy.describe())
            .prose(texts::tr!("app.updates.report.beforeInstalling"))
            .mail(to, subject)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_daemon_state_is_read_leniently() {
        let r: DaemonReport = serde_json::from_value(serde_json::json!({
            "phase": "prepared",
            "target": { "version": "1.3.0", "package": "/x", "api": "a", "web": "w" },
            "installed_version": "1.2.0",
            "available": { "version": "1.3.0", "notes": "faster" },
            "failed": ["1.2.5"],
            "last": {
                "kind": "rolled_back", "version": "1.2.5", "from": "1.2.0",
                "detail": "did not answer", "at": "2026-10-18T07:00:00Z"
            },
            "handled_request": "p1",
            "phase_since": "2026-10-18T07:00:00Z",
            "progress": "2 flujos corriendo",
            "something_new": 1
        }))
        .unwrap();
        assert_eq!(r.phase, "prepared");
        assert_eq!(r.installed, "1.2.0");
        assert_eq!(r.target.unwrap().version, "1.3.0");
        assert_eq!(r.available.unwrap().notes, "faster");
        assert_eq!(r.last.unwrap().kind, "rolled_back");
        assert_eq!(r.handled_request.as_deref(), Some("p1"));
        assert_eq!(r.progress.as_deref(), Some("2 flujos corriendo"));
        assert!(r.phase_since.is_some());
        // An empty one does not fail.
        let r: DaemonReport = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(r, DaemonReport::default());
    }

    #[test]
    fn each_outcome_has_its_mail_except_a_postponement() {
        let outcome = |kind: &str| Outcome {
            kind: kind.into(),
            version: "1.3.0".into(),
            from: "1.2.0".into(),
            detail: "the detail".into(),
            at: Utc::now(),
        };
        for kind in ["updated", "rolled_back", "restored", "critical", "failed"] {
            let mail = outcome(kind).mail(vec!["a@b.co".into()]).unwrap();
            assert!(mail.text.contains("the detail"), "{kind}");
        }
        assert!(outcome("critical")
            .mail(vec![])
            .unwrap()
            .subject
            .starts_with("URGENTE"));
        assert_eq!(outcome("postponed").mail(vec![]), None);
    }
}
