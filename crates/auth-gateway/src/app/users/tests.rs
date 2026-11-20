use super::*;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use crate::{
    AccountStatus, LockoutState, NewAdmin, PasswordHashError, PasswordHasher, PasswordPolicy,
    PasswordResetStore, ResetTokenGenerator,
};
use async_trait::async_trait;
use domain::Clock;

use crate::app::mail::{MailSlot, Mailer, OutgoingMail};

#[derive(Default)]
struct Accounts(Mutex<Vec<AdminAccount>>);

#[async_trait]
impl AdminRepository for Accounts {
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
    async fn insert(&self, _: &NewAdmin) -> Result<AdminId, RepositoryError> {
        unreachable!()
    }
    async fn list_accounts(&self) -> Result<Vec<AccountSummary>, RepositoryError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|a| AccountSummary {
                id: a.id.clone(),
                email: a.email.clone(),
                display_name: a.display_name.clone(),
                role: a.role.clone(),
                status: AccountStatus::of(&a.password_hash),
                created_at: SystemTime::UNIX_EPOCH,
            })
            .collect())
    }
    async fn create_invited(
        &self,
        email: &Email,
        display_name: Option<&str>,
        role: &Role,
    ) -> Result<AdminId, RepositoryError> {
        let mut all = self.0.lock().unwrap();
        if all.iter().any(|a| &a.email == email) {
            return Err(RepositoryError::EmailTaken);
        }
        let id = AdminId::new(format!("id-{email}"));
        all.push(AdminAccount {
            id: id.clone(),
            email: email.clone(),
            password_hash: PasswordHash::pending_activation(),
            lockout: LockoutState::clear(),
            role: role.clone(),
            display_name: display_name.map(str::to_string),
        });
        Ok(id)
    }
    async fn delete_account(&self, id: &AdminId) -> Result<bool, RepositoryError> {
        let mut all = self.0.lock().unwrap();
        let before = all.len();
        all.retain(|a| &a.id != id);
        Ok(all.len() < before)
    }
    async fn update_account(
        &self,
        id: &AdminId,
        display_name: Option<&str>,
        role: &Role,
    ) -> Result<bool, RepositoryError> {
        let mut all = self.0.lock().unwrap();
        let Some(a) = all.iter_mut().find(|a| &a.id == id) else {
            return Ok(false);
        };
        a.display_name = display_name.map(str::to_string);
        a.role = role.clone();
        Ok(true)
    }
}

#[derive(Default)]
struct Links(Mutex<HashMap<String, AdminId>>);

#[async_trait]
impl PasswordResetStore for Links {
    async fn issue(
        &self,
        admin: &AdminId,
        token_hash: &str,
        _: SystemTime,
    ) -> Result<(), RepositoryError> {
        let mut links = self.0.lock().unwrap();
        links.retain(|_, a| a != admin);
        links.insert(token_hash.to_string(), admin.clone());
        Ok(())
    }
    async fn find(&self, h: &str, _: SystemTime) -> Result<Option<AdminId>, RepositoryError> {
        Ok(self.0.lock().unwrap().get(h).cloned())
    }
    async fn redeem(
        &self,
        h: &str,
        _: &PasswordHash,
        _: SystemTime,
    ) -> Result<Option<AdminId>, RepositoryError> {
        Ok(self.0.lock().unwrap().remove(h))
    }
}

struct Tokens;
impl ResetTokenGenerator for Tokens {
    fn generate(&self) -> String {
        "tok".into()
    }
    fn hash(&self, token: &str) -> String {
        format!("h:{token}")
    }
}

struct Hasher;
#[async_trait]
impl PasswordHasher for Hasher {
    async fn hash(&self, p: &str) -> Result<PasswordHash, PasswordHashError> {
        Ok(PasswordHash::from_encoded(p))
    }
    async fn verify(&self, _: &str, _: &PasswordHash) -> Result<bool, PasswordHashError> {
        Ok(false)
    }
    fn reference_hash(&self) -> PasswordHash {
        PasswordHash::from_encoded("ref")
    }
}

struct Now;
impl Clock for Now {
    fn now(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000)
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

fn service(with_mail: bool) -> (UsersService, Arc<Accounts>, Arc<Outbox>) {
    let accounts = Arc::new(Accounts::default());
    let outbox = Arc::new(Outbox::default());
    let mail = if with_mail {
        MailSlot::fixed(outbox.clone())
    } else {
        MailSlot::default()
    };
    let resets = PasswordResetService::new(
        accounts.clone(),
        Arc::new(Links::default()),
        Arc::new(Tokens),
        Arc::new(Hasher),
        Arc::new(Now),
        mail,
        PasswordPolicy::recommended(),
        Duration::from_secs(900),
    );
    (
        UsersService::new(accounts.clone(), resets),
        accounts,
        outbox,
    )
}

fn ana() -> NewUser {
    NewUser {
        email: "Ana@Empresa.com".into(),
        display_name: Some("  Ana Ruiz ".into()),
        role: "user".into(),
    }
}

#[tokio::test]
async fn creating_a_user_leaves_it_pending_and_emails_the_activation_link() {
    let (svc, accounts, outbox) = service(true);
    let created = svc.create(ana(), "https://orq.example.com/").await.unwrap();
    assert!(created.activation_sent);
    let id = created.id;

    let account = accounts.find_by_id(&id).await.unwrap().unwrap();
    assert!(account.password_hash.is_pending_activation());
    assert_eq!(account.display_name.as_deref(), Some("Ana Ruiz"));
    let sent = outbox.0.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, vec!["ana@empresa.com".to_string()]);
    assert!(sent[0]
        .text
        .contains("https://orq.example.com/activate#tok"));
    assert!(sent[0].text.contains("72 horas"));
}

#[tokio::test]
async fn nothing_is_created_without_outgoing_mail() {
    let (svc, accounts, _) = service(false);
    assert!(matches!(
        svc.create(ana(), "https://x").await,
        Err(UsersError::MailUnavailable)
    ));
    assert!(accounts.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn create_validates_email_role_and_duplicates() {
    let (svc, _, _) = service(true);
    let bad_email = NewUser {
        email: "no-es-correo".into(),
        ..ana()
    };
    assert!(matches!(
        svc.create(bad_email, "https://x").await,
        Err(UsersError::Invalid("email", _))
    ));
    let bad_role = NewUser {
        role: "superuser".into(),
        ..ana()
    };
    assert!(matches!(
        svc.create(bad_role, "https://x").await,
        Err(UsersError::Invalid("role", _))
    ));
    svc.create(ana(), "https://x").await.unwrap();
    assert!(matches!(
        svc.create(ana(), "https://x").await,
        Err(UsersError::EmailTaken)
    ));
}

#[tokio::test]
async fn only_a_pending_account_gets_its_link_again() {
    let (svc, accounts, outbox) = service(true);
    let id = svc.create(ana(), "https://x").await.unwrap().id;
    svc.resend(&id, "https://x").await.unwrap();
    assert_eq!(outbox.0.lock().unwrap().len(), 2);

    accounts.0.lock().unwrap()[0].password_hash = PasswordHash::from_encoded("$argon2id$x");
    assert!(matches!(
        svc.resend(&id, "https://x").await,
        Err(UsersError::NotPending)
    ));
    assert!(matches!(
        svc.resend(&AdminId::new("nope"), "https://x").await,
        Err(UsersError::NotFound)
    ));
}

fn changes(name: Option<&str>, role: &str) -> UserChanges {
    UserChanges {
        display_name: name.map(str::to_string),
        role: role.into(),
    }
}

#[tokio::test]
async fn update_changes_name_and_role() {
    let (svc, accounts, _) = service(true);
    let boss = svc
        .create(
            NewUser {
                email: "jefa@empresa.com".into(),
                display_name: None,
                role: "admin".into(),
            },
            "https://x",
        )
        .await
        .unwrap()
        .id;
    let ana = svc.create(ana(), "https://x").await.unwrap().id;

    let email = svc
        .update(&boss, &ana, changes(Some("  Ana R. "), "admin"))
        .await
        .unwrap();
    assert_eq!(email.as_str(), "ana@empresa.com");
    let account = accounts.find_by_id(&ana).await.unwrap().unwrap();
    assert_eq!(account.display_name.as_deref(), Some("Ana R."));
    assert_eq!(account.role, Role::admin());

    // With two administrators, one can stop being one; the last cannot.
    svc.update(&boss, &ana, changes(None, "user"))
        .await
        .unwrap();
    let account = accounts.find_by_id(&ana).await.unwrap().unwrap();
    assert_eq!(account.display_name, None);
    assert!(matches!(
        svc.update(&ana, &boss, changes(None, "user")).await,
        Err(UsersError::LastAdmin)
    ));
}

#[tokio::test]
async fn update_refuses_ones_own_role_bad_input_and_directory_accounts() {
    let (svc, accounts, _) = service(true);
    let boss = svc
        .create(
            NewUser {
                email: "jefa@empresa.com".into(),
                display_name: None,
                role: "admin".into(),
            },
            "https://x",
        )
        .await
        .unwrap()
        .id;
    assert!(matches!(
        svc.update(&boss, &boss, changes(None, "user")).await,
        Err(UsersError::SelfRole)
    ));
    // Their own name, yes.
    svc.update(&boss, &boss, changes(Some("Jefa"), "admin"))
        .await
        .unwrap();
    assert!(matches!(
        svc.update(&boss, &boss, changes(None, "root")).await,
        Err(UsersError::Invalid("role", _))
    ));
    assert!(matches!(
        svc.update(&boss, &AdminId::new("nope"), changes(None, "user"))
            .await,
        Err(UsersError::NotFound)
    ));

    let ana = svc.create(ana(), "https://x").await.unwrap().id;
    accounts.0.lock().unwrap()[1].password_hash = PasswordHash::directory_managed();
    assert!(matches!(
        svc.update(&boss, &ana, changes(Some("Ana Ruiz"), "admin"))
            .await,
        Err(UsersError::DirectoryManaged)
    ));
    // Without changes, it does no harm (the form also saves the areas).
    svc.update(&boss, &ana, changes(Some("Ana Ruiz"), "user"))
        .await
        .unwrap();
}

#[tokio::test]
async fn delete_refuses_oneself_and_the_last_admin() {
    let (svc, _, _) = service(true);
    let boss = svc
        .create(
            NewUser {
                email: "jefa@empresa.com".into(),
                display_name: None,
                role: "admin".into(),
            },
            "https://x",
        )
        .await
        .unwrap()
        .id;
    let ana = svc.create(ana(), "https://x").await.unwrap().id;

    assert!(matches!(
        svc.delete(&boss, &boss).await,
        Err(UsersError::SelfDelete)
    ));
    assert!(matches!(
        svc.delete(&ana, &boss).await,
        Err(UsersError::LastAdmin)
    ));
    assert_eq!(
        svc.delete(&boss, &ana).await.unwrap().as_str(),
        "ana@empresa.com"
    );
    assert!(matches!(
        svc.delete(&boss, &ana).await,
        Err(UsersError::NotFound)
    ));
}

#[tokio::test]
async fn whoever_is_not_admin_cannot_make_an_admin_whatever_the_case() {
    let (svc, _, _) = service(true);
    for role in ["admin", "Admin", " ADMIN ", "aDmIn"] {
        assert!(
            matches!(
                svc.guard_admin(false, None, Some(role)).await,
                Err(UsersError::AdminOnly)
            ),
            "{role:?} passed the guard"
        );
    }
    assert!(svc.guard_admin(false, None, Some("user")).await.is_ok());
    assert!(svc.guard_admin(true, None, Some("Admin")).await.is_ok());
}

#[test]
fn a_service_accounts_token_never_acts_as_admin() {
    use crate::http::session::CurrentSession;
    let session = |service_account: Option<&str>| CurrentSession {
        admin_id: "owner".into(),
        role: Role::admin(),
        token: crate::SessionToken::from_raw(""),
        service_account: service_account.map(Into::into),
    };
    assert!(session(None).acts_as_admin());
    assert!(!session(Some("robot")).acts_as_admin());
}
