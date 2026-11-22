use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::Utc;
use domain::{AuditEvent, AuditId, NewAuditEvent, RepositoryError};

use super::*;
use crate::settings::StoredSection;

/// The sections by name, and who saved each.
#[derive(Default)]
struct Store(Mutex<BTreeMap<String, (serde_json::Value, String)>>);

#[async_trait]
impl SettingsStore for Store {
    async fn load(&self, section: &str) -> Result<Option<StoredSection>, String> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(section)
            .map(|(data, _)| StoredSection {
                data: data.clone(),
                updated_at: Utc::now(),
                updated_by: None,
            }))
    }

    async fn save(
        &self,
        section: &str,
        data: serde_json::Value,
        updated_by: &str,
    ) -> Result<StoredSection, String> {
        let saved = (data.clone(), updated_by.to_string());
        self.0.lock().unwrap().insert(section.into(), saved);
        Ok(StoredSection {
            data,
            updated_at: Utc::now(),
            updated_by: None,
        })
    }

    async fn delete(&self, section: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(section);
        Ok(())
    }
}

#[derive(Default)]
struct Audit(Mutex<Vec<NewActionAudit>>);

#[async_trait]
impl AuditRepository for Audit {
    async fn record(&self, _: &NewAuditEvent) -> Result<AuditId, RepositoryError> {
        Ok(AuditId::new("a"))
    }

    async fn record_action(&self, event: &NewActionAudit) -> Result<AuditId, RepositoryError> {
        self.0.lock().unwrap().push(event.clone());
        Ok(AuditId::new("a"))
    }

    async fn list_recent(&self, _: u32) -> Result<Vec<AuditEvent>, RepositoryError> {
        Ok(Vec::new())
    }
}

fn command() -> (PlatformTimezone, Arc<Store>, Arc<Audit>) {
    let (store, audit) = (Arc::new(Store::default()), Arc::new(Audit::default()));
    (
        PlatformTimezone::new(store.clone(), audit.clone()),
        store,
        audit,
    )
}

#[tokio::test]
async fn never_set_it_is_utc() {
    let (tz, _, _) = command();
    assert_eq!(tz.get().await.unwrap(), "UTC");
}

#[tokio::test]
async fn a_known_zone_is_saved_with_no_person_and_audited_as_the_command() {
    let (tz, store, audit) = command();
    let saved = tz
        .set(" America/Bogota ", "server timezone set")
        .await
        .unwrap();
    assert_eq!(saved, "America/Bogota");
    assert_eq!(tz.get().await.unwrap(), "America/Bogota");
    let (data, by) = store.0.lock().unwrap()["platform"].clone();
    assert_eq!(data, serde_json::json!({ "timezone": "America/Bogota" }));
    assert_eq!(by, "", "no person signed in");
    let events = audit.0.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, AuditEventType::ResourceChanged);
    assert_eq!(events[0].admin_id, None);
    assert_eq!(events[0].action, iam::catalog::CONFIGURE);
    assert_eq!(events[0].resource, "settings/platform");
    assert_eq!(events[0].user_agent.as_deref(), Some("server timezone set"));
}

#[tokio::test]
async fn an_unknown_zone_changes_nothing() {
    let (tz, store, audit) = command();
    tz.set("Europe/Madrid", "c").await.unwrap();
    let err = tz.set("Mars/Olympus", "c").await.unwrap_err();
    assert!(matches!(err, SettingsError::Invalid(f) if f.contains_key("timezone")));
    assert_eq!(tz.get().await.unwrap(), "Europe/Madrid");
    assert_eq!(store.0.lock().unwrap().len(), 1);
    assert_eq!(audit.0.lock().unwrap().len(), 1);
}
