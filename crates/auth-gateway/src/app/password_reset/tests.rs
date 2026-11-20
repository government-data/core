use super::*;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::SystemTime;

use crate::{AdminAccount, LockoutState, NewAdmin, PasswordHash, PasswordHashError};
use async_trait::async_trait;
use domain::{RepositoryError, Role};

use crate::app::account_mail::spoken;
use crate::app::mail::{Mailer, OutgoingMail};

struct Admins(Mutex<Vec<AdminAccount>>);

#[async_trait]
impl AdminRepository for Admins {
    async fn find_by_email(&self, email: &Email) -> Result<Option<AdminAccount>, RepositoryError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .find(|a| &a.email == email)
            .cloned())
    }
    async fn find_by_id(&self, id: &AdminId) -> Result<Option<AdminAccount>, RepositoryError> {
        Ok(self.0.lock().unwrap().iter().find(|a| &a.id == id).cloned())
    }
    async fn update_lockout(&self, _: &AdminId, _: &LockoutState) -> Result<(), RepositoryError> {
        Ok(())
    }
    async fn count(&self) -> Result<u64, RepositoryError> {
        Ok(self.0.lock().unwrap().len() as u64)
    }
    async fn insert(&self, new: &NewAdmin) -> Result<AdminId, RepositoryError> {
        let mut admins = self.0.lock().unwrap();
        let id = AdminId::new((admins.len() + 1).to_string());
        admins.push(AdminAccount {
            id: id.clone(),
            email: new.email.clone(),
            password_hash: new.password_hash.clone(),
            lockout: LockoutState::clear(),
            role: new.role.clone(),
            display_name: None,
        });
        Ok(id)
    }
    // mia@example.com chose English in the UI.
    async fn locale(&self, id: &AdminId) -> Result<Option<String>, RepositoryError> {
        Ok((*id == AdminId::new("3")).then(|| "en".to_string()))
    }
}

/// token hash -> (admin, expires_at); plus the passwords set.
#[derive(Default)]
struct Store {
    links: Mutex<HashMap<String, (AdminId, SystemTime)>>,
    passwords: Mutex<HashMap<AdminId, String>>,
}

#[async_trait]
impl PasswordResetStore for Store {
    async fn issue(
        &self,
        admin: &AdminId,
        hash: &str,
        at: SystemTime,
    ) -> Result<(), RepositoryError> {
        let mut links = self.links.lock().unwrap();
        links.retain(|_, (a, _)| a != admin);
        links.insert(hash.to_string(), (admin.clone(), at));
        Ok(())
    }
    async fn find(&self, hash: &str, now: SystemTime) -> Result<Option<AdminId>, RepositoryError> {
        Ok(self
            .links
            .lock()
            .unwrap()
            .get(hash)
            .filter(|(_, at)| *at > now)
            .map(|(a, _)| a.clone()))
    }
    async fn redeem(
        &self,
        hash: &str,
        new: &PasswordHash,
        now: SystemTime,
    ) -> Result<Option<AdminId>, RepositoryError> {
        let Some((admin, at)) = self.links.lock().unwrap().remove(hash) else {
            return Ok(None);
        };
        if at <= now {
            return Ok(None);
        }
        self.passwords
            .lock()
            .unwrap()
            .insert(admin.clone(), new.as_str().to_string());
        Ok(Some(admin))
    }
}

struct Tokens;

impl ResetTokenGenerator for Tokens {
    fn generate(&self) -> String {
        "secret-token".into()
    }
    fn hash(&self, token: &str) -> String {
        format!("h({token})")
    }
}

struct Hasher;

#[async_trait]
impl PasswordHasher for Hasher {
    async fn hash(&self, plain: &str) -> Result<PasswordHash, PasswordHashError> {
        Ok(PasswordHash::from_encoded(format!("argon({plain})")))
    }
    async fn verify(&self, _: &str, _: &PasswordHash) -> Result<bool, PasswordHashError> {
        Ok(false)
    }
    fn reference_hash(&self) -> PasswordHash {
        PasswordHash::from_encoded("ref")
    }
}

struct Clock(Mutex<SystemTime>);

impl domain::Clock for Clock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap()
    }
}

#[derive(Default)]
struct Outbox(Mutex<Vec<OutgoingMail>>);

#[async_trait]
impl Mailer for Outbox {
    async fn send(&self, mail: &OutgoingMail) -> Result<(), String> {
        self.0.lock().unwrap().push(mail.clone());
        Ok(())
    }
}

fn account(id: &str, email: &str, hash: PasswordHash) -> AdminAccount {
    AdminAccount {
        id: AdminId::new(id),
        email: Email::parse(email).unwrap(),
        password_hash: hash,
        lockout: LockoutState::clear(),
        role: Role::admin(),
        display_name: Some("Ana".into()),
    }
}

struct Fixture {
    svc: PasswordResetService,
    store: Arc<Store>,
    outbox: Arc<Outbox>,
    clock: Arc<Clock>,
}

fn fixture(mail: bool) -> Fixture {
    fixture_with(
        mail,
        vec![
            account(
                "1",
                "ana@example.com",
                PasswordHash::from_encoded("argon(old)"),
            ),
            account("2", "luis@corp.example", PasswordHash::directory_managed()),
            account(
                "3",
                "mia@example.com",
                PasswordHash::from_encoded("argon(old)"),
            ),
        ],
    )
}

fn fixture_with(mail: bool, admins: Vec<AdminAccount>) -> Fixture {
    let store = Arc::new(Store::default());
    let outbox = Arc::new(Outbox::default());
    let clock = Arc::new(Clock(Mutex::new(SystemTime::UNIX_EPOCH)));
    let slot = if mail {
        MailSlot::fixed(outbox.clone())
    } else {
        MailSlot::default()
    };
    let svc = PasswordResetService::new(
        Arc::new(Admins(Mutex::new(admins))),
        store.clone(),
        Arc::new(Tokens),
        Arc::new(Hasher),
        clock.clone(),
        slot,
        PasswordPolicy::recommended(),
        Duration::from_secs(30 * 60),
    );
    Fixture {
        svc,
        store,
        outbox,
        clock,
    }
}

/// Let the background send run.
async fn sent(outbox: &Outbox, n: usize) -> Vec<OutgoingMail> {
    for _ in 0..50 {
        if outbox.0.lock().unwrap().len() >= n {
            break;
        }
        tokio::task::yield_now().await;
    }
    outbox.0.lock().unwrap().clone()
}

#[tokio::test]
async fn a_local_account_gets_a_link_that_opens_the_form_and_expires() {
    let f = fixture(true);
    let who = f
        .svc
        .request(" Ana@Example.com ", "https://orq.example.com/")
        .await
        .unwrap();
    assert_eq!(who, Some(AdminId::new("1")));

    let mail = sent(&f.outbox, 1).await;
    assert_eq!(mail[0].to, vec!["ana@example.com".to_string()]);
    assert!(mail[0].text.contains("Hola, Ana:"));
    assert!(mail[0]
        .text
        .contains("https://orq.example.com/reset-password#secret-token"));
    assert!(mail[0].text.contains("vence en 30 minutos"));
    // Only the hash is stored.
    assert!(f
        .store
        .links
        .lock()
        .unwrap()
        .contains_key("h(secret-token)"));

    assert!(f.svc.check("secret-token").await.unwrap());
    *f.clock.0.lock().unwrap() += Duration::from_secs(30 * 60);
    assert!(!f.svc.check("secret-token").await.unwrap());
    assert!(matches!(
        f.svc.reset("secret-token", "N3wPasswordOk").await,
        Err(ResetError::InvalidToken)
    ));
}

#[tokio::test]
async fn a_link_sets_the_new_password_once() {
    let f = fixture(true);
    f.svc
        .request("ana@example.com", "https://orq")
        .await
        .unwrap();

    let id = f.svc.reset("secret-token", "N3wPasswordOk").await.unwrap();
    assert_eq!(id, AdminId::new("1"));
    assert_eq!(
        f.store
            .passwords
            .lock()
            .unwrap()
            .get(&id)
            .map(String::as_str),
        Some("argon(N3wPasswordOk)")
    );
    assert!(matches!(
        f.svc.reset("secret-token", "An0therPassword").await,
        Err(ResetError::InvalidToken)
    ));
}

#[tokio::test]
async fn the_account_hears_its_password_changed() {
    let f = fixture(true);
    f.svc
        .request("ana@example.com", "https://orq")
        .await
        .unwrap();
    f.svc.reset("secret-token", "N3wPasswordOk").await.unwrap();
    let mail = sent(&f.outbox, 2).await;
    assert_eq!(mail.len(), 2, "el enlace y el aviso");
    assert_eq!(mail[1].to, ["ana@example.com"]);
    assert_eq!(
        mail[1].subject,
        "La contraseña de tu cuenta de Quality Data cambió"
    );
    assert!(mail[1].text.contains("Si no fuiste tú"), "{}", mail[1].text);
    assert!(mail[1].html.as_deref().unwrap().contains("Hola, Ana:"));
}

#[tokio::test]
async fn an_activated_account_gets_a_welcome_instead() {
    let pending = account("4", "eva@example.com", PasswordHash::pending_activation());
    let f = fixture_with(true, vec![pending.clone()]);
    f.svc
        .send_activation(&pending, "https://orq")
        .await
        .unwrap();
    f.svc.reset("secret-token", "N3wPasswordOk").await.unwrap();
    let mail = sent(&f.outbox, 2).await;
    assert_eq!(mail[1].subject, "Tu cuenta de Quality Data está activa");
}

#[tokio::test]
async fn a_deleted_account_is_told_unless_it_never_signed_in() {
    let f = fixture(true);
    let ana = account(
        "1",
        "ana@example.com",
        PasswordHash::from_encoded("argon(old)"),
    );
    f.svc.notify_deleted(&ana, texts::Language::En).await;
    let pending = account("4", "eva@example.com", PasswordHash::pending_activation());
    f.svc.notify_deleted(&pending, texts::Language::Es).await;
    let mail = sent(&f.outbox, 1).await;
    assert_eq!(mail.len(), 1);
    assert_eq!(mail[0].subject, "Your Quality Data account was deleted");
}

#[tokio::test]
async fn a_weak_password_is_refused_and_keeps_the_link() {
    let f = fixture(true);
    f.svc
        .request("ana@example.com", "https://orq")
        .await
        .unwrap();

    assert!(matches!(
        f.svc.reset("secret-token", "short").await,
        Err(ResetError::WeakPassword(_))
    ));
    assert!(f.svc.check("secret-token").await.unwrap());
}

#[tokio::test]
async fn an_unknown_email_answers_the_same_and_sends_nothing() {
    let f = fixture(true);
    assert_eq!(
        f.svc
            .request("nadie@example.com", "https://orq")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        f.svc.request("not-an-email", "https://orq").await.unwrap(),
        None
    );
    assert!(sent(&f.outbox, 1).await.is_empty());
}

#[tokio::test]
async fn a_directory_account_is_told_to_change_it_in_the_directory() {
    let f = fixture(true);
    f.svc
        .request("luis@corp.example", "https://orq")
        .await
        .unwrap();
    let mail = sent(&f.outbox, 1).await;
    assert!(mail[0].text.contains("Active Directory"));
    assert!(!mail[0].text.contains("reset-password"));
    assert!(f.store.links.lock().unwrap().is_empty());
}

#[tokio::test]
async fn without_outgoing_mail_there_is_no_recovery() {
    let f = fixture(false);
    assert!(matches!(
        f.svc.request("ana@example.com", "https://orq").await,
        Err(ResetError::MailUnavailable)
    ));
}

#[test]
fn the_expiry_reads_naturally() {
    assert_eq!(spoken(Duration::from_secs(60)), "1 minuto");
    assert_eq!(spoken(Duration::from_secs(45 * 60)), "45 minutos");
    assert_eq!(spoken(Duration::from_secs(3600)), "1 hora");
    assert_eq!(spoken(Duration::from_secs(2 * 3600)), "2 horas");
}

#[tokio::test]
async fn the_reset_mail_goes_in_the_account_language_or_the_companys() {
    let f = fixture(true);
    f.svc
        .request("mia@example.com", "https://orq.example.com")
        .await
        .unwrap();
    let mail = sent(&f.outbox, 1).await;
    assert_eq!(mail[0].subject, "Reset your Quality Data password");
    assert!(mail[0].text.contains("expires in 30 minutes"));

    // Without its own language: the tenant's notice language.
    f.svc.mail.set_notice_language(texts::Language::En);
    f.svc
        .request("luis@corp.example", "https://orq.example.com")
        .await
        .unwrap();
    let mail = sent(&f.outbox, 2).await;
    assert!(mail[1].text.contains("company directory"));
}

#[tokio::test]
async fn the_activation_mail_goes_in_the_company_notice_language() {
    let f = fixture(true);
    let mia = account(
        "3",
        "mia@example.com",
        PasswordHash::from_encoded("argon(old)"),
    );
    f.svc
        .send_activation(&mia, "https://orq.example.com")
        .await
        .unwrap();
    f.svc.mail.set_notice_language(texts::Language::En);
    f.svc
        .send_activation(&mia, "https://orq.example.com")
        .await
        .unwrap();

    let mail = f.outbox.0.lock().unwrap().clone();
    assert_eq!(mail[0].subject, "Activa tu cuenta de Quality Data");
    assert!(mail[0].text.contains("vence en 72 horas"));
    assert_eq!(mail[1].subject, "Activate your Quality Data account");
    assert!(mail[1].text.contains("expires in 72 hours"));
    assert!(mail[1]
        .text
        .contains("https://orq.example.com/activate#secret-token"));
}

#[tokio::test]
async fn the_first_admin_is_created_pending_with_an_activation_link() {
    let f = fixture_with(true, vec![]);
    let access = f
        .svc
        .admin_access(" ti@cliente.co ", "https://orq.cliente.co/", false)
        .await
        .unwrap();
    assert!(access.created);
    assert!(access.emailed);
    assert_eq!(
        access.link.as_deref(),
        Some("https://orq.cliente.co/activate#secret-token")
    );
    let mail = f.outbox.0.lock().unwrap().clone();
    assert_eq!(mail[0].to, vec!["ti@cliente.co".to_string()]);
    assert!(mail[0].text.contains("/activate#secret-token"));

    // Again (the link expired, or the install is repeated): the same account,
    // still pending, gets a new one.
    let again = f
        .svc
        .admin_access("ti@cliente.co", "https://orq.cliente.co", false)
        .await
        .unwrap();
    assert!(!again.created);
    assert_eq!(again.admin, access.admin);
    assert!(again.link.is_some());
    assert_eq!(f.store.links.lock().unwrap().len(), 1);

    // They choose their password with the link.
    f.svc
        .reset("secret-token", "Correct horse battery 9")
        .await
        .unwrap();
    assert!(f
        .store
        .passwords
        .lock()
        .unwrap()
        .contains_key(&access.admin));
}

#[tokio::test]
async fn an_account_with_a_password_gets_a_link_only_when_forced() {
    let f = fixture(false);
    let access = f
        .svc
        .admin_access("ana@example.com", "https://orq.example.com", false)
        .await
        .unwrap();
    assert_eq!(access.link, None);
    assert!(f.store.links.lock().unwrap().is_empty());

    let forced = f
        .svc
        .admin_access("ana@example.com", "https://orq.example.com", true)
        .await
        .unwrap();
    assert_eq!(
        forced.link.as_deref(),
        Some("https://orq.example.com/reset-password#secret-token")
    );
    // Without outgoing mail: only the link, which the server gives.
    assert!(!forced.emailed);
}

#[tokio::test]
async fn no_admin_access_for_a_stranger_or_a_directory_account() {
    let f = fixture(true);
    assert!(matches!(
        f.svc
            .admin_access("otro@example.com", "https://x", true)
            .await,
        Err(AccessError::UnknownAccount)
    ));
    assert!(matches!(
        f.svc
            .admin_access("luis@corp.example", "https://x", true)
            .await,
        Err(AccessError::DirectoryAccount)
    ));
    assert!(matches!(
        f.svc.admin_access("no es correo", "https://x", true).await,
        Err(AccessError::InvalidEmail(_))
    ));
}
