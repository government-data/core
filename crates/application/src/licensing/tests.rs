use super::*;
use crate::licensing::AdminContacts;
use crate::settings::OutgoingMail;
use license_model::StoredLicense;
use std::sync::Mutex;

struct Cloud {
    answer: Mutex<Result<Entitlement, FetchError>>,
    /// The heartbeats received: key, installation and whom to warn.
    beats: Mutex<Vec<(String, String, Vec<String>)>>,
    /// Whether the cloud answers the billing.
    billing_up: Mutex<bool>,
}

#[async_trait::async_trait]
impl LicenseAuthority for Cloud {
    async fn fetch(
        &self,
        _key: &str,
        _installation: &str,
        _usage: Option<&license_model::Usage>,
    ) -> Result<String, FetchError> {
        let answer = self.answer.lock().unwrap().clone()?;
        Ok(serde_json::to_string(&answer).unwrap())
    }
    fn verify(&self, signed: &str) -> Result<Entitlement, String> {
        serde_json::from_str(signed).map_err(|_| "invalid signature".to_string())
    }
    async fn heartbeat(
        &self,
        key: &str,
        installation: &str,
        notify: &[String],
    ) -> Result<(), String> {
        self.beats
            .lock()
            .unwrap()
            .push((key.into(), installation.into(), notify.to_vec()));
        Ok(())
    }
    async fn billing(
        &self,
        key: &str,
        installation: &str,
    ) -> Result<license_model::Billing, String> {
        if !*self.billing_up.lock().unwrap() {
            return Err("no hay conexión con la nube de licencias".into());
        }
        assert_eq!((key, installation), ("ORQ-PRUEBA-0001", "inst-1"));
        Ok(license_model::Billing {
            period: Some("anual".into()),
            packages: Default::default(),
            renewal_cop: Some(18_000_000),
            payments: vec![],
        })
    }
}

#[derive(Default)]
struct Memory(Mutex<StoredLicense>, Mutex<Vec<String>>);

#[async_trait::async_trait]
impl LicenseStore for Memory {
    async fn load(&self) -> Result<StoredLicense, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    async fn save_key(&self, key: &str) -> Result<(), String> {
        let mut s = self.0.lock().unwrap();
        s.key = Some(key.into());
        s.permit = None;
        Ok(())
    }
    async fn save_permit(&self, permit: &str) -> Result<(), String> {
        self.0.lock().unwrap().permit = Some(permit.into());
        Ok(())
    }
    async fn record_error(&self, error: &str) -> Result<(), String> {
        self.0.lock().unwrap().last_error = Some(error.into());
        Ok(())
    }
    async fn clear_permit(&self, error: &str) -> Result<(), String> {
        let mut s = self.0.lock().unwrap();
        s.permit = None;
        s.last_error = Some(error.into());
        Ok(())
    }
    async fn reminders_sent(&self) -> Result<Vec<String>, String> {
        Ok(self.1.lock().unwrap().clone())
    }
    async fn mark_reminders(&self, sent: &[String]) -> Result<(), String> {
        *self.1.lock().unwrap() = sent.to_vec();
        Ok(())
    }
}

#[derive(Default)]
struct Outbox(Mutex<Vec<OutgoingMail>>);

#[async_trait::async_trait]
impl crate::settings::Mailer for Outbox {
    async fn send(&self, mail: &OutgoingMail) -> Result<(), String> {
        self.0.lock().unwrap().push(mail.clone());
        Ok(())
    }
}

struct Admins;

#[async_trait::async_trait]
impl AdminContacts for Admins {
    async fn admin_emails(&self) -> Result<Vec<String>, String> {
        Ok(vec!["ti@cooperativa.coop".into()])
    }
}

#[tokio::test]
async fn the_contract_end_is_announced_once_per_step_and_then_read_only() {
    let ends = Utc::now() + chrono::Duration::days(45);
    let (license, _) = service(Ok(Entitlement {
        contract_ends_at: Some(ends),
        ..plan("inst-1", 30)
    }));
    let outbox = Arc::new(Outbox::default());
    let license = license.with_reminders(MailSlot::fixed(outbox.clone()), Arc::new(Admins));
    license.refresh().await.unwrap();
    assert!(license.read_only().is_none());

    let day = |d: i64| ends - chrono::Duration::days(d);
    // 45 days before: the 60-day one.
    let sent = license.send_reminders(day(45)).await.unwrap();
    assert!(sent.unwrap().ends_with(":60d"));
    // The same day again, or the next: nothing.
    assert_eq!(license.send_reminders(day(44)).await.unwrap(), None);
    // At 29 days: the 30-day one; at 5: the 7-day one.
    assert!(license
        .send_reminders(day(29))
        .await
        .unwrap()
        .unwrap()
        .ends_with(":30d"));
    assert!(license
        .send_reminders(day(5))
        .await
        .unwrap()
        .unwrap()
        .ends_with(":7d"));
    // It ends: the grace notice; after the grace period: read-only.
    let after = |d: i64| ends + chrono::Duration::days(d);
    assert!(license
        .send_reminders(after(1))
        .await
        .unwrap()
        .unwrap()
        .ends_with(":expired"));
    let last = license
        .send_reminders(after(license_model::CONTRACT_GRACE_DAYS + 1))
        .await
        .unwrap();
    assert!(last.unwrap().ends_with(":read-only"));
    let mails = outbox.0.lock().unwrap();
    assert_eq!(mails.len(), 5);
    assert_eq!(mails[0].to, ["ti@cooperativa.coop"]);
    assert!(
        mails[4].subject.contains("solo lectura"),
        "{}",
        mails[4].subject
    );
}

#[tokio::test]
async fn an_overdue_or_suspended_contract_makes_it_read_only() {
    let (overdue, _) = service(Ok(Entitlement {
        contract_ends_at: Some(Utc::now() - chrono::Duration::days(20)),
        ..plan("inst-1", 30)
    }));
    overdue.refresh().await.unwrap();
    assert!(overdue.read_only().unwrap().contains("venció"));
    // The plan limits stay: read-only is not the basic plan.
    assert_eq!(overdue.limits().workflows, Some(150));

    let (suspended, _) = service(Ok(Entitlement {
        suspended: Some("factura FE-1 pendiente".into()),
        ..plan("inst-1", 30)
    }));
    suspended.refresh().await.unwrap();
    assert!(suspended.read_only().unwrap().contains("FE-1"));
    assert!(matches!(
        suspended.view().contract,
        Some(ContractStatus::ReadOnly { .. })
    ));
}

fn plan(installation: &str, days: i64) -> Entitlement {
    Entitlement {
        customer: "Empresa".into(),
        plan: "Profesional".into(),
        features: vec!["python".into()],
        limits: Limits {
            workflows: Some(150),
            ..Limits::default()
        },
        installation: installation.into(),
        issued_at: Utc::now(),
        expires_at: Utc::now() + chrono::Duration::days(days),
        contract_ends_at: None,
        suspended: None,
    }
}

fn service(answer: Result<Entitlement, FetchError>) -> (LicenseService, Arc<Cloud>) {
    let cloud = Arc::new(Cloud {
        answer: Mutex::new(answer),
        beats: Mutex::default(),
        billing_up: Mutex::new(true),
    });
    let service = LicenseService::new(
        cloud.clone(),
        Arc::new(Memory::default()),
        "inst-1".into(),
        Some("ORQ-PRUEBA-0001".into()),
        true,
    );
    (service, cloud)
}

#[tokio::test]
async fn billing_comes_from_the_cloud_and_the_last_one_stays_if_it_goes_silent() {
    let (license, cloud) = service(Ok(plan("inst-1", 30)));
    let view = license.billing().await.unwrap();
    assert_eq!(view.billing.unwrap().renewal_cop, Some(18_000_000));
    assert!(view.error.is_none());

    *cloud.billing_up.lock().unwrap() = false;
    let view = license.billing().await.unwrap();
    assert_eq!(view.billing.unwrap().period.as_deref(), Some("anual"));
    assert!(view.fetched_at.is_some());
    assert!(view.error.unwrap().contains("no hay conexión"));
}

#[tokio::test]
async fn the_heartbeat_tells_the_cloud_whom_to_warn_if_it_goes_silent() {
    let (license, cloud) = service(Ok(plan("inst-1", 30)));
    // Without anyone to warn configured, the heartbeat goes anyway (no emails).
    license.heartbeat().await.unwrap();
    let license = license.with_reminders(
        MailSlot::fixed(Arc::new(Outbox::default())),
        Arc::new(Admins),
    );
    license.heartbeat().await.unwrap();
    assert_eq!(
        *cloud.beats.lock().unwrap(),
        [
            ("ORQ-PRUEBA-0001".into(), "inst-1".into(), vec![]),
            (
                "ORQ-PRUEBA-0001".into(),
                "inst-1".into(),
                vec!["ti@cooperativa.coop".into()]
            ),
        ]
    );
}

#[tokio::test]
async fn without_a_permit_it_is_basic_and_a_refresh_activates_the_plan() {
    let (license, _) = service(Ok(plan("inst-1", 30)));
    license.load().await.unwrap();
    assert!(matches!(license.status(), LicenseStatus::Basic { .. }));
    assert_eq!(license.limits(), Limits::BASIC);
    assert!(!license.has_feature("python"));

    license.refresh().await.unwrap();
    assert_eq!(license.status(), LicenseStatus::Active);
    assert_eq!(license.limits().workflows, Some(150));
    assert!(license.has_feature("python"));
    assert_eq!(license.view().key_hint.as_deref(), Some("ORQ-PRUE…"));
}

#[tokio::test]
async fn a_failed_renewal_keeps_the_last_good_permit() {
    let (license, cloud) = service(Ok(plan("inst-1", 30)));
    license.refresh().await.unwrap();
    *cloud.answer.lock().unwrap() = Err(FetchError::Unreachable("sin conexión".into()));
    assert!(license.refresh().await.is_err());
    assert_eq!(license.status(), LicenseStatus::Active);
    assert_eq!(license.view().last_error.as_deref(), Some("sin conexión"));
}

#[tokio::test]
async fn a_revoked_key_loses_its_permit_at_once() {
    let (license, cloud) = service(Ok(plan("inst-1", 30)));
    license.refresh().await.unwrap();
    *cloud.answer.lock().unwrap() = Err(FetchError::Rejected("la clave fue revocada".into()));
    assert!(license.refresh().await.is_err());
    assert!(matches!(license.status(), LicenseStatus::Basic { .. }));
    assert_eq!(license.limits(), Limits::BASIC);
}

#[tokio::test]
async fn a_permit_for_another_installation_is_refused() {
    let (license, _) = service(Ok(plan("otra", 30)));
    assert!(license.refresh().await.is_err());
    assert!(matches!(license.status(), LicenseStatus::Basic { .. }));
}
