use std::sync::Mutex;

use chrono::{DateTime, Utc};

use super::policy::UpdateMode;
use super::*;

/// The updates store, in memory.
#[derive(Default)]
pub(crate) struct MemoryMaintenance {
    pub(crate) current: Mutex<Option<Maintenance>>,
    pub(crate) timezone: Mutex<Option<String>>,
    pub(crate) row: Mutex<UpdaterRow>,
    pub(crate) history: Mutex<Vec<HistoryEntry>>,
    pub(crate) modules: Mutex<Vec<InstalledModule>>,
}

#[async_trait::async_trait]
impl UpdatesStore for MemoryMaintenance {
    async fn maintenance(&self) -> Result<Option<Maintenance>, String> {
        Ok(self.current.lock().unwrap().clone())
    }
    async fn begin_maintenance(
        &self,
        reason: &str,
        until: DateTime<Utc>,
    ) -> Result<Maintenance, String> {
        let mut current = self.current.lock().unwrap();
        let now = Utc::now();
        let started_at = current
            .as_ref()
            .filter(|m| m.active_at(now))
            .map_or(now, |m| m.started_at);
        let m = Maintenance {
            reason: reason.into(),
            started_at,
            until,
        };
        *current = Some(m.clone());
        Ok(m)
    }
    async fn end_maintenance(&self) -> Result<(), String> {
        let now = Utc::now();
        if let Some(m) = self.current.lock().unwrap().as_mut() {
            if m.active_at(now) {
                m.until = now;
            }
        }
        Ok(())
    }
    async fn updater(&self) -> Result<UpdaterRow, String> {
        Ok(self.row.lock().unwrap().clone())
    }
    async fn platform_timezone(&self) -> Result<Option<String>, String> {
        Ok(self.timezone.lock().unwrap().clone())
    }
    async fn save_modules(&self, modules: &[InstalledModule]) -> Result<(), String> {
        *self.modules.lock().unwrap() = modules.to_vec();
        Ok(())
    }
    async fn modules(&self) -> Result<Vec<InstalledModule>, String> {
        Ok(self.modules.lock().unwrap().clone())
    }
    async fn save_report(&self, report: &serde_json::Value) -> Result<(), String> {
        self.row.lock().unwrap().report = Some((report.clone(), Utc::now()));
        Ok(())
    }
    async fn save_policy(&self, policy: &serde_json::Value, _admin: &str) -> Result<(), String> {
        self.row.lock().unwrap().policy =
            Some((policy.clone(), Utc::now(), Some("admin@coop.co".into())));
        Ok(())
    }
    async fn new_request(&self, _admin: &str) -> Result<UpdateRequest, String> {
        let mut row = self.row.lock().unwrap();
        let n = row.request.as_ref().map_or(0, |r| r.id.len());
        let request = UpdateRequest {
            id: format!("request-{n}"),
            at: Utc::now(),
            by: Some("admin@coop.co".into()),
        };
        row.request = Some(request.clone());
        Ok(request)
    }
    async fn mark_notified(
        &self,
        available: Option<&str>,
        result: Option<&str>,
    ) -> Result<(), String> {
        let mut row = self.row.lock().unwrap();
        if let Some(a) = available {
            row.notified_available = Some(a.into());
        }
        if let Some(r) = result {
            row.notified_result = Some(r.into());
        }
        Ok(())
    }
    async fn add_history(&self, entry: &HistoryEntry) -> Result<(), String> {
        let mut h = self.history.lock().unwrap();
        if !h
            .iter()
            .any(|e| e.at == entry.at && e.version == entry.version && e.kind == entry.kind)
        {
            h.insert(0, entry.clone());
        }
        Ok(())
    }
    async fn history(&self, limit: u32) -> Result<Vec<HistoryEntry>, String> {
        Ok(self
            .history
            .lock()
            .unwrap()
            .iter()
            .take(limit as usize)
            .cloned()
            .collect())
    }
}

/// The mailer, writing down what was sent.
#[derive(Default)]
struct Outbox(
    Mutex<Vec<crate::OutgoingMail>>,
    std::sync::atomic::AtomicBool,
);

#[async_trait::async_trait]
impl crate::Mailer for Outbox {
    async fn send(&self, mail: &crate::OutgoingMail) -> Result<(), String> {
        if self.1.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("the mail server does not answer".into());
        }
        self.0.lock().unwrap().push(mail.clone());
        Ok(())
    }
}

struct Admins;

#[async_trait::async_trait]
impl AdminContacts for Admins {
    async fn admin_emails(&self) -> Result<Vec<String>, String> {
        Ok(vec!["admin@coop.co".into()])
    }
}

fn with_mail() -> (UpdatesService, Arc<MemoryMaintenance>, Arc<Outbox>) {
    let store = Arc::new(MemoryMaintenance::default());
    let outbox = Arc::new(Outbox::default());
    let updates = UpdatesService::new(store.clone())
        .with_notices(MailSlot::fixed(outbox.clone()), Arc::new(Admins));
    (updates, store, outbox)
}

fn report(phase: &str, available: Option<&str>, last: Option<(&str, &str)>) -> serde_json::Value {
    let mut r = serde_json::json!({
        "phase": phase,
        "installed_version": "1.2.0",
        "failed": [],
    });
    if let Some(v) = available {
        r["available"] = serde_json::json!({ "version": v, "notes": "what is new" });
    }
    if let Some((kind, at)) = last {
        r["last"] = serde_json::json!({
            "kind": kind, "version": "1.3.0", "from": "1.2.0",
            "detail": "the detail", "at": at,
        });
    }
    r
}

#[tokio::test]
async fn a_new_version_and_each_outcome_are_mailed_once() {
    let (updates, store, outbox) = with_mail();
    updates
        .report(report("prepared", Some("1.3.0"), None))
        .await
        .unwrap();
    updates
        .report(report("prepared", Some("1.3.0"), None))
        .await
        .unwrap();
    {
        let sent = outbox.0.lock().unwrap();
        assert_eq!(sent.len(), 1, "the new version, once");
        assert!(sent[0].subject.contains("1.3.0"));
        assert!(
            sent[0].text.contains("Actualizar"),
            "it says how to install it"
        );
    }
    let at = "2026-10-18T07:00:00Z";
    for _ in 0..3 {
        updates
            .report(report("idle", None, Some(("rolled_back", at))))
            .await
            .unwrap();
    }
    assert_eq!(outbox.0.lock().unwrap().len(), 2, "the outcome, once");
    assert_eq!(
        store.history.lock().unwrap().len(),
        1,
        "and once in the history"
    );
    // A postponement is not announced.
    updates
        .report(report(
            "prepared",
            None,
            Some(("postponed", "2026-10-19T07:00:00Z")),
        ))
        .await
        .unwrap();
    assert_eq!(outbox.0.lock().unwrap().len(), 2);
    assert_eq!(store.history.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_mail_that_failed_is_retried_on_the_next_report() {
    let (updates, _, outbox) = with_mail();
    outbox.1.store(true, std::sync::atomic::Ordering::SeqCst);
    updates
        .report(report("prepared", Some("1.3.0"), None))
        .await
        .unwrap();
    assert!(outbox.0.lock().unwrap().is_empty());
    outbox.1.store(false, std::sync::atomic::Ordering::SeqCst);
    updates
        .report(report("prepared", Some("1.3.0"), None))
        .await
        .unwrap();
    assert_eq!(outbox.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_request_is_handed_to_the_daemon_until_it_handles_it() {
    let (updates, _, _) = with_mail();
    // Without a daemon, there is nobody to ask.
    assert!(matches!(
        updates.request_now("a").await,
        Err(UpdatesError::Conflict(_))
    ));
    updates
        .report(report("prepared", Some("1.3.0"), None))
        .await
        .unwrap();
    let request = updates.request_now("a").await.unwrap();
    let orders = updates.orders().await.unwrap();
    assert_eq!(orders["request"]["id"], request.id);
    assert_eq!(orders["policy"]["mode"], "approval");
    assert_eq!(
        updates.view().await.unwrap().pending_request,
        Some(request.clone())
    );

    // The daemon handled it: it is no longer handed out.
    let mut r = report("draining", Some("1.3.0"), None);
    r["handled_request"] = serde_json::json!(request.id);
    updates.report(r).await.unwrap();
    assert_eq!(
        updates.orders().await.unwrap()["request"],
        serde_json::Value::Null
    );
    // And with one in progress another is not asked for.
    assert!(matches!(
        updates.request_now("a").await,
        Err(UpdatesError::Conflict(_))
    ));
}

#[tokio::test]
async fn the_policy_is_validated_and_read_back() {
    let (updates, _, _) = with_mail();
    let mut p = UpdatePolicy {
        mode: UpdateMode::Automatic,
        ..UpdatePolicy::default()
    };
    p.window.days = vec![];
    assert!(matches!(
        updates.set_policy("a", p.clone()).await,
        Err(UpdatesError::Invalid("window", _))
    ));
    p.window.days = vec![6, 7];
    updates.set_policy("a", p.clone()).await.unwrap();
    let view = updates.view().await.unwrap();
    assert_eq!(view.policy, p);
    assert_eq!(view.policy_updated_by.as_deref(), Some("admin@coop.co"));
    assert_eq!(
        updates.orders().await.unwrap()["policy"]["window"]["days"],
        serde_json::json!([6, 7])
    );
}

#[tokio::test]
async fn the_window_in_the_orders_is_in_the_platforms_time_zone() {
    let (updates, store, _) = with_mail();
    let mut p = UpdatePolicy::default();
    p.window.timezone = "America/Bogota".into();
    updates.set_policy("a", p).await.unwrap();
    let zone = || async { updates.orders().await.unwrap()["policy"]["window"]["timezone"].clone() };
    assert_eq!(
        zone().await,
        "UTC",
        "never saved: UTC, not the window's own"
    );

    *store.timezone.lock().unwrap() = Some("Europe/Madrid".into());
    assert_eq!(zone().await, "Europe/Madrid");

    *store.timezone.lock().unwrap() = Some("Mars/Olympus".into());
    assert_eq!(zone().await, "UTC", "one that does not parse: UTC");
    assert_eq!(
        updates.view().await.unwrap().policy.window.timezone,
        "America/Bogota",
        "the stored window is not rewritten"
    );
}

#[tokio::test]
async fn everyone_reads_the_platforms_time_zone_or_utc() {
    let (updates, store, _) = with_mail();
    let zone = || async { updates.platform_timezone().await.unwrap() };
    assert_eq!(zone().await, "UTC", "never saved");
    *store.timezone.lock().unwrap() = Some("Europe/Madrid".into());
    assert_eq!(zone().await, "Europe/Madrid");
    *store.timezone.lock().unwrap() = Some("Mars/Olympus".into());
    assert_eq!(zone().await, "UTC", "does not parse");
}

#[tokio::test]
async fn the_page_says_when_the_daemon_stopped_reporting() {
    let (updates, store, _) = with_mail();
    assert!(!updates.view().await.unwrap().daemon);
    store.row.lock().unwrap().report = Some((
        report("idle", None, None),
        Utc::now() - chrono::Duration::minutes(20),
    ));
    let view = updates.view().await.unwrap();
    assert!(view.daemon);
    assert!(view.stale);
    assert_eq!(view.report.installed, "1.2.0");
}

fn service() -> (UpdatesService, Arc<MemoryMaintenance>) {
    let store = Arc::new(MemoryMaintenance::default());
    (UpdatesService::new(store.clone()), store)
}

#[tokio::test]
async fn begin_and_end() {
    let (updates, _) = service();
    assert_eq!(updates.active().await.unwrap(), None);
    let m = updates
        .begin("actualizando a 1.3.0", Duration::from_secs(600))
        .await
        .unwrap();
    assert_eq!(updates.active().await.unwrap(), Some(m.clone()));

    // Renewing it keeps when it started.
    let again = updates
        .begin("actualizando a 1.3.0", Duration::from_secs(1200))
        .await
        .unwrap();
    assert_eq!(again.started_at, m.started_at);
    assert!(again.until > m.until);

    updates.end().await.unwrap();
    assert_eq!(updates.active().await.unwrap(), None);
    // Just ended, it still counts so silence is not taken as an outage.
    assert!(updates.within(Duration::from_secs(300)).await.unwrap());
}

#[tokio::test]
async fn it_expires_on_its_own() {
    let (updates, store) = service();
    *store.current.lock().unwrap() = Some(Maintenance {
        reason: "a daemon that died".into(),
        started_at: Utc::now() - chrono::Duration::hours(3),
        until: Utc::now() - chrono::Duration::hours(1),
    });
    assert_eq!(updates.active().await.unwrap(), None);
    assert!(!updates.within(Duration::from_secs(300)).await.unwrap());
}

#[tokio::test]
async fn it_needs_a_reason_and_a_sane_length() {
    let (updates, _) = service();
    assert!(updates.begin("  ", Duration::from_secs(60)).await.is_err());
    assert!(updates.begin("x", Duration::ZERO).await.is_err());
    assert!(updates
        .begin("x", MAX_MAINTENANCE + Duration::from_secs(1))
        .await
        .is_err());
}

#[test]
fn drained_means_nothing_half_done() {
    let mut d = DrainStatus {
        queued: 3,
        scheduled_soon: 2,
        ..Default::default()
    };
    assert!(d.is_drained(), "what is queued waits; nothing is half done");
    d.running = 1;
    assert!(!d.is_drained());
    d.running = 0;
    d.deploying = 1;
    assert!(!d.is_drained());
    d.deploying = 0;
    d.not_answering = vec!["silent".into()];
    assert!(
        !d.is_drained(),
        "a module that does not answer may be running"
    );
}

/// Two modules that answer and one that does not.
struct Modules;

#[async_trait::async_trait]
impl ModulesWork for Modules {
    async fn work(&self, horizon: Duration) -> Vec<(String, Result<ModuleWork, String>)> {
        assert_eq!(horizon, Duration::from_secs(60));
        let w = |running, queued, scheduled_soon| ModuleWork {
            running,
            queued,
            scheduled_soon,
        };
        vec![
            ("a".into(), Ok(w(1, 2, 0))),
            ("b".into(), Ok(w(0, 1, 3))),
            ("c".into(), Err("timed out".into())),
        ]
    }
}

#[tokio::test]
async fn the_drain_adds_every_modules_work_and_names_the_silent_ones() {
    let (updates, _) = service();
    let nothing = updates.drain_status(Duration::from_secs(60)).await;
    assert!(nothing.is_drained(), "without modules nothing runs");
    let updates = updates.with_modules_work(Arc::new(Modules));
    let d = updates.drain_status(Duration::from_secs(60)).await;
    assert_eq!(
        (d.running, d.deploying, d.queued, d.scheduled_soon),
        (1, 0, 3, 3)
    );
    assert_eq!(d.not_answering, ["c"]);
    assert!(!d.is_drained());
}
