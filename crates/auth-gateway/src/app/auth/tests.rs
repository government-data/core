use super::*;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use crate::{AdminAccount, LockoutState, PasswordHash, PasswordHashError};
use domain::RepositoryError;

// A hasher whose "hash" is just the plaintext prefixed, so verification is a
// string compare — enough to exercise the use case without real argon2.
#[derive(Default)]
struct FakeHasher;

#[async_trait::async_trait]
impl PasswordHasher for FakeHasher {
    async fn hash(&self, plaintext: &str) -> Result<PasswordHash, PasswordHashError> {
        Ok(PasswordHash::from_encoded(format!("hash:{plaintext}")))
    }
    async fn verify(
        &self,
        plaintext: &str,
        hash: &PasswordHash,
    ) -> Result<bool, PasswordHashError> {
        // Like argon2: a marker (`!directory`, `!pending`) is no hash.
        if hash.as_str().starts_with('!') {
            return Err(PasswordHashError("corrupt hash".into()));
        }
        Ok(hash.as_str() == format!("hash:{plaintext}"))
    }
    fn reference_hash(&self) -> PasswordHash {
        PasswordHash::from_encoded("hash:__reference__")
    }
}

#[derive(Default)]
struct InMemoryRepo {
    accounts: Mutex<Vec<AdminAccount>>,
}

#[async_trait::async_trait]
impl AdminRepository for InMemoryRepo {
    async fn find_by_email(&self, email: &Email) -> Result<Option<AdminAccount>, RepositoryError> {
        Ok(self
            .accounts
            .lock()
            .unwrap()
            .iter()
            .find(|a| &a.email == email)
            .cloned())
    }
    async fn find_by_id(&self, id: &AdminId) -> Result<Option<AdminAccount>, RepositoryError> {
        Ok(self
            .accounts
            .lock()
            .unwrap()
            .iter()
            .find(|a| &a.id == id)
            .cloned())
    }
    async fn update_lockout(
        &self,
        id: &AdminId,
        state: &LockoutState,
    ) -> Result<(), RepositoryError> {
        for a in self.accounts.lock().unwrap().iter_mut() {
            if &a.id == id {
                a.lockout = *state;
            }
        }
        Ok(())
    }
    async fn count(&self) -> Result<u64, RepositoryError> {
        Ok(self.accounts.lock().unwrap().len() as u64)
    }
    async fn insert(&self, admin: &NewAdmin) -> Result<AdminId, RepositoryError> {
        let mut accounts = self.accounts.lock().unwrap();
        if accounts.iter().any(|a| a.email == admin.email) {
            return Err(RepositoryError::EmailTaken);
        }
        let id = AdminId::new(format!("id-{}", accounts.len() + 1));
        accounts.push(AdminAccount {
            id: id.clone(),
            email: admin.email.clone(),
            password_hash: admin.password_hash.clone(),
            lockout: LockoutState::clear(),
            role: admin.role.clone(),
            display_name: None,
        });
        Ok(id)
    }
}

#[derive(Default)]
struct InMemoryIpStore {
    entries: Mutex<HashMap<String, LockoutState>>,
}

#[async_trait::async_trait]
impl IpLockoutStore for InMemoryIpStore {
    async fn get(&self, ip: &str) -> Result<LockoutState, RepositoryError> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .get(ip)
            .copied()
            .unwrap_or_default())
    }
    async fn put(&self, ip: &str, state: &LockoutState) -> Result<(), RepositoryError> {
        self.entries.lock().unwrap().insert(ip.to_string(), *state);
        Ok(())
    }
}

// A clock the test can advance by hand.
struct FixedClock(Mutex<SystemTime>);
impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap()
    }
}

fn seed_admin(repo: &InMemoryRepo, email: &str, password: &str) {
    repo.accounts.lock().unwrap().push(AdminAccount {
        id: AdminId::new("id-1"),
        email: Email::parse(email).unwrap(),
        password_hash: PasswordHash::from_encoded(format!("hash:{password}")),
        lockout: LockoutState::clear(),
        role: Role::admin(),
        display_name: None,
    });
}

fn service(
    repo: Arc<InMemoryRepo>,
    ips: Arc<InMemoryIpStore>,
    clock: Arc<FixedClock>,
    policy: LockoutPolicy,
) -> LoginService {
    LoginService::new(repo, ips, Arc::new(FakeHasher), clock, policy)
}

fn req(email: &str, password: &str, ip: &str) -> LoginRequest {
    LoginRequest {
        email: email.to_string(),
        password: password.to_string(),
        client_ip: ip.to_string(),
    }
}

#[tokio::test]
async fn an_account_waiting_for_activation_signs_in_like_an_unknown_email() {
    let repo = Arc::new(InMemoryRepo::default());
    repo.accounts.lock().unwrap().push(AdminAccount {
        id: AdminId::new("id-1"),
        email: Email::parse("ana@example.com").unwrap(),
        password_hash: PasswordHash::pending_activation(),
        lockout: LockoutState::clear(),
        role: Role::admin(),
        display_name: None,
    });
    let clock = Arc::new(FixedClock(Mutex::new(SystemTime::UNIX_EPOCH)));
    let svc = service(
        repo,
        Arc::new(InMemoryIpStore::default()),
        clock,
        LockoutPolicy::recommended(),
    );
    // Not an internal error (the marker is no hash), and no password —
    // not even the marker itself — opens it.
    for password in ["", "!pending", "anything"] {
        assert!(matches!(
            svc.login(req("ana@example.com", password, "10.0.0.1"))
                .await,
            Err(LoginError::InvalidCredentials)
        ));
    }
}

#[tokio::test]
async fn correct_password_authenticates() {
    let repo = Arc::new(InMemoryRepo::default());
    seed_admin(&repo, "admin@example.com", "correct horse");
    let clock = Arc::new(FixedClock(Mutex::new(SystemTime::UNIX_EPOCH)));
    let svc = service(
        repo,
        Arc::new(InMemoryIpStore::default()),
        clock,
        LockoutPolicy::recommended(),
    );

    let authenticated = svc
        .login(req("admin@example.com", "correct horse", "10.0.0.1"))
        .await
        .unwrap();
    assert_eq!(authenticated.id.as_str(), "id-1");
    assert_eq!(authenticated.role, Role::admin());
}

#[tokio::test]
async fn wrong_password_and_unknown_user_are_indistinguishable() {
    let repo = Arc::new(InMemoryRepo::default());
    seed_admin(&repo, "admin@example.com", "correct horse");
    let clock = Arc::new(FixedClock(Mutex::new(SystemTime::UNIX_EPOCH)));
    let svc = service(
        repo,
        Arc::new(InMemoryIpStore::default()),
        clock,
        LockoutPolicy::recommended(),
    );

    let wrong = svc
        .login(req("admin@example.com", "nope", "10.0.0.1"))
        .await;
    let unknown = svc
        .login(req("ghost@example.com", "nope", "10.0.0.2"))
        .await;
    assert!(matches!(wrong, Err(LoginError::InvalidCredentials)));
    assert!(matches!(unknown, Err(LoginError::InvalidCredentials)));
}

#[tokio::test]
async fn account_locks_after_threshold_then_rejects_even_correct_password() {
    let repo = Arc::new(InMemoryRepo::default());
    seed_admin(&repo, "admin@example.com", "correct horse");
    let clock = Arc::new(FixedClock(Mutex::new(SystemTime::UNIX_EPOCH)));
    let policy = LockoutPolicy {
        max_attempts: 3,
        base_delay: Duration::from_secs(60),
        max_delay: Duration::from_secs(600),
    };
    // Use a distinct IP per attempt so the IP lock does not mask the account
    // lock we are exercising.
    let svc = service(repo, Arc::new(InMemoryIpStore::default()), clock, policy);

    for i in 0..3 {
        let e = svc
            .login(req("admin@example.com", "wrong", &format!("10.0.0.{i}")))
            .await;
        assert!(matches!(e, Err(LoginError::InvalidCredentials)));
    }
    // Now locked: even the correct password is refused.
    let locked = svc
        .login(req("admin@example.com", "correct horse", "10.0.0.9"))
        .await;
    assert!(matches!(locked, Err(LoginError::TooManyAttempts { .. })));
}

#[derive(Default)]
struct Outbox(Mutex<Vec<crate::OutgoingMail>>);

#[async_trait::async_trait]
impl crate::Mailer for Outbox {
    async fn send(&self, mail: &crate::OutgoingMail) -> Result<(), String> {
        self.0.lock().unwrap().push(mail.clone());
        Ok(())
    }
}

#[tokio::test]
async fn the_account_hears_once_when_it_locks() {
    let repo = Arc::new(InMemoryRepo::default());
    seed_admin(&repo, "admin@example.com", "correct horse");
    let clock = Arc::new(FixedClock(Mutex::new(SystemTime::UNIX_EPOCH)));
    let policy = LockoutPolicy {
        max_attempts: 3,
        base_delay: Duration::from_secs(60),
        max_delay: Duration::from_secs(600),
    };
    let outbox = Arc::new(Outbox::default());
    let svc = service(repo, Arc::new(InMemoryIpStore::default()), clock, policy)
        .with_mail(MailSlot::fixed(outbox.clone()));

    for i in 0..4 {
        let _ = svc
            .login(req("admin@example.com", "wrong", &format!("10.0.0.{i}")))
            .await;
    }
    let _ = svc
        .login(req("nadie@example.com", "wrong", "10.0.0.9"))
        .await;
    for _ in 0..50 {
        if !outbox.0.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
    let mail = outbox.0.lock().unwrap().clone();
    assert_eq!(mail.len(), 1, "solo al bloquearse");
    assert_eq!(mail[0].to, ["admin@example.com"]);
    assert!(mail[0].text.contains("3 intentos"), "{}", mail[0].text);
    assert!(mail[0].text.contains("1 minuto"), "{}", mail[0].text);
}

#[tokio::test]
async fn ip_locks_out_across_different_emails() {
    let repo = Arc::new(InMemoryRepo::default());
    let clock = Arc::new(FixedClock(Mutex::new(SystemTime::UNIX_EPOCH)));
    let policy = LockoutPolicy {
        max_attempts: 3,
        base_delay: Duration::from_secs(60),
        max_delay: Duration::from_secs(600),
    };
    let svc = service(repo, Arc::new(InMemoryIpStore::default()), clock, policy);

    // Three misses from one IP against nonexistent accounts still throttle it.
    for i in 0..3 {
        let _ = svc
            .login(req(&format!("ghost{i}@example.com"), "x", "203.0.113.7"))
            .await;
    }
    let blocked = svc
        .login(req("another@example.com", "x", "203.0.113.7"))
        .await;
    assert!(matches!(blocked, Err(LoginError::TooManyAttempts { .. })));
}

#[tokio::test]
async fn success_after_failures_clears_counters() {
    let repo = Arc::new(InMemoryRepo::default());
    seed_admin(&repo, "admin@example.com", "correct horse");
    let clock = Arc::new(FixedClock(Mutex::new(SystemTime::UNIX_EPOCH)));
    let policy = LockoutPolicy {
        max_attempts: 5,
        base_delay: Duration::from_secs(60),
        max_delay: Duration::from_secs(600),
    };
    let ips = Arc::new(InMemoryIpStore::default());
    let svc = service(repo, ips.clone(), clock, policy);

    for _ in 0..2 {
        let _ = svc
            .login(req("admin@example.com", "wrong", "198.51.100.1"))
            .await;
    }
    // A success resets the IP counter to the cleared state.
    svc.login(req("admin@example.com", "correct horse", "198.51.100.1"))
        .await
        .unwrap();
    assert_eq!(
        ips.get("198.51.100.1").await.unwrap(),
        LockoutState::clear()
    );
}

#[tokio::test]
async fn bootstrap_creates_then_is_idempotent_and_enforces_policy() {
    let repo = Arc::new(InMemoryRepo::default());
    let svc = BootstrapService::new(
        repo.clone(),
        Arc::new(FakeHasher),
        PasswordPolicy::recommended(),
    );

    // Weak password rejected before any write.
    assert!(matches!(
        svc.create_first_admin("admin@example.com", "short").await,
        Err(BootstrapError::WeakPassword(_))
    ));
    assert_eq!(repo.count().await.unwrap(), 0);

    // First strong bootstrap creates the admin.
    let outcome = svc
        .create_first_admin("admin@example.com", "Str0ngEnoughPass")
        .await
        .unwrap();
    assert!(matches!(outcome, BootstrapOutcome::Created(_)));

    // A second bootstrap no-ops rather than resetting anything.
    let again = svc
        .create_first_admin("other@example.com", "Str0ngEnoughPass")
        .await
        .unwrap();
    assert_eq!(again, BootstrapOutcome::AlreadyInitialized);
    assert_eq!(repo.count().await.unwrap(), 1);
}

// --- Directory (LDAP) sign-in ------------------------------------------

/// A directory with one person: `ana@corp.example` / `ldap-pass`.
struct FakeDirectory {
    groups: Vec<String>,
}

#[async_trait::async_trait]
impl DirectoryAuthenticator for FakeDirectory {
    async fn authenticate(
        &self,
        login: &str,
        password: &str,
    ) -> Result<Option<crate::DirectoryUser>, crate::DirectoryError> {
        Ok(
            (login == "ana@corp.example" && password == "ldap-pass").then(|| {
                crate::DirectoryUser {
                    username: "ana".into(),
                    email: Email::parse("ana@corp.example").unwrap(),
                    display_name: Some("Ana".into()),
                    groups: self.groups.clone(),
                }
            }),
        )
    }
}

/// Upserts into the same in-memory repo the login reads from, and records
/// the last group sync.
struct FakeDirectoryAccounts {
    repo: Arc<InMemoryRepo>,
    mapped: Vec<String>,
    synced: Mutex<Option<Vec<String>>>,
}

#[async_trait::async_trait]
impl DirectoryAccounts for FakeDirectoryAccounts {
    async fn grants_access(&self, groups: &[String]) -> Result<bool, RepositoryError> {
        Ok(groups.iter().any(|g| self.mapped.contains(g)))
    }
    async fn upsert(
        &self,
        user: &crate::DirectoryUser,
        role: &Role,
    ) -> Result<AdminAccount, RepositoryError> {
        let mut accounts = self.repo.accounts.lock().unwrap();
        if let Some(a) = accounts.iter_mut().find(|a| a.email == user.email) {
            if !a.password_hash.is_directory_managed() {
                return Err(RepositoryError::EmailTaken);
            }
            a.role = role.clone();
            return Ok(a.clone());
        }
        let account = AdminAccount {
            id: AdminId::new(format!("dir-{}", accounts.len() + 1)),
            email: user.email.clone(),
            password_hash: PasswordHash::directory_managed(),
            lockout: LockoutState::clear(),
            role: role.clone(),
            display_name: user.display_name.clone(),
        };
        accounts.push(account.clone());
        Ok(account)
    }
    async fn sync_groups(&self, _: &AdminId, groups: &[String]) -> Result<(), RepositoryError> {
        *self.synced.lock().unwrap() = Some(groups.to_vec());
        Ok(())
    }
}

const AREA_GROUP: &str = "CN=KNIME-Finanzas,OU=Grupos,DC=corp";
const ADMIN_GROUP: &str = "CN=KNIME-Admins,OU=Grupos,DC=corp";

fn directory_service(
    repo: Arc<InMemoryRepo>,
    ips: Arc<InMemoryIpStore>,
    groups: &[&str],
) -> (LoginService, Arc<FakeDirectoryAccounts>) {
    let accounts = Arc::new(FakeDirectoryAccounts {
        repo: repo.clone(),
        mapped: vec![AREA_GROUP.to_string()],
        synced: Mutex::new(None),
    });
    let clock = Arc::new(FixedClock(Mutex::new(SystemTime::UNIX_EPOCH)));
    let svc =
        service(repo, ips, clock, LockoutPolicy::recommended()).with_directory(DirectoryLogin {
            authenticator: Arc::new(FakeDirectory {
                groups: groups.iter().map(|g| g.to_string()).collect(),
            }),
            accounts: accounts.clone(),
            admin_groups: vec![ADMIN_GROUP.to_lowercase()],
        });
    (svc, accounts)
}

#[tokio::test]
async fn directory_user_with_a_mapped_group_signs_in_as_user() {
    let repo = Arc::new(InMemoryRepo::default());
    let (svc, accounts) = directory_service(
        repo.clone(),
        Arc::new(InMemoryIpStore::default()),
        &[AREA_GROUP],
    );

    let who = svc
        .login(req("ana@corp.example", "ldap-pass", "10.0.0.1"))
        .await
        .unwrap();
    assert_eq!(who.role.as_str(), DIRECTORY_USER_ROLE);
    let stored = repo
        .find_by_id(&who.id)
        .await
        .unwrap()
        .expect("the local account was created");
    assert!(stored.password_hash.is_directory_managed());
    assert_eq!(
        accounts.synced.lock().unwrap().as_deref(),
        Some(&[AREA_GROUP.to_string()][..])
    );

    // A second sign-in reuses the account instead of creating another.
    let again = svc
        .login(req("ana@corp.example", "ldap-pass", "10.0.0.1"))
        .await
        .unwrap();
    assert_eq!(again.id, who.id);
}

#[tokio::test]
async fn an_admin_group_grants_the_admin_role_in_any_case() {
    let (svc, _) = directory_service(
        Arc::new(InMemoryRepo::default()),
        Arc::new(InMemoryIpStore::default()),
        &[ADMIN_GROUP],
    );
    let who = svc
        .login(req("ana@corp.example", "ldap-pass", "10.0.0.1"))
        .await
        .unwrap();
    assert_eq!(who.role, Role::admin());
}

#[tokio::test]
async fn a_directory_user_without_mapped_groups_is_refused() {
    let repo = Arc::new(InMemoryRepo::default());
    let (svc, _) = directory_service(
        repo.clone(),
        Arc::new(InMemoryIpStore::default()),
        &["CN=Everyone,DC=corp"],
    );
    let e = svc
        .login(req("ana@corp.example", "ldap-pass", "10.0.0.1"))
        .await;
    assert!(matches!(e, Err(LoginError::InvalidCredentials)));
    assert_eq!(
        repo.count().await.unwrap(),
        0,
        "no local account is created"
    );
}

#[tokio::test]
async fn a_wrong_directory_password_counts_against_the_ip() {
    let ips = Arc::new(InMemoryIpStore::default());
    let (svc, _) = directory_service(
        Arc::new(InMemoryRepo::default()),
        ips.clone(),
        &[AREA_GROUP],
    );
    let e = svc
        .login(req("ana@corp.example", "wrong", "10.0.0.7"))
        .await;
    assert!(matches!(e, Err(LoginError::InvalidCredentials)));
    assert_eq!(ips.get("10.0.0.7").await.unwrap().failed_attempts, 1);
}

/// A directory that cannot be reached (server down, timeout).
struct DownDirectory;

#[async_trait::async_trait]
impl DirectoryAuthenticator for DownDirectory {
    async fn authenticate(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Option<crate::DirectoryUser>, crate::DirectoryError> {
        Err(crate::DirectoryError("connection refused".into()))
    }
}

#[tokio::test]
async fn an_unreachable_directory_is_reported_as_such_and_not_as_a_failure() {
    let repo = Arc::new(InMemoryRepo::default());
    let ips = Arc::new(InMemoryIpStore::default());
    let clock = Arc::new(FixedClock(Mutex::new(SystemTime::UNIX_EPOCH)));
    let svc = service(
        repo.clone(),
        ips.clone(),
        clock,
        LockoutPolicy::recommended(),
    )
    .with_directory(DirectoryLogin {
        authenticator: Arc::new(DownDirectory),
        accounts: Arc::new(FakeDirectoryAccounts {
            repo,
            mapped: vec![AREA_GROUP.to_string()],
            synced: Mutex::new(None),
        }),
        admin_groups: vec![ADMIN_GROUP.to_lowercase()],
    });

    let e = svc
        .login(req("ana@corp.example", "ldap-pass", "10.0.0.8"))
        .await;
    assert!(
        matches!(&e, Err(LoginError::DirectoryUnavailable(msg)) if msg == "connection refused"),
        "got {e:?}"
    );
    assert_eq!(
        ips.get("10.0.0.8").await.unwrap().failed_attempts,
        0,
        "an outage is not the caller's failed attempt"
    );
}

#[tokio::test]
async fn local_accounts_keep_using_their_password_with_a_directory_configured() {
    let repo = Arc::new(InMemoryRepo::default());
    seed_admin(&repo, "admin@example.com", "correct horse");
    let (svc, _) = directory_service(repo, Arc::new(InMemoryIpStore::default()), &[AREA_GROUP]);
    let who = svc
        .login(req("admin@example.com", "correct horse", "10.0.0.1"))
        .await
        .unwrap();
    assert_eq!(who.id.as_str(), "id-1");
    // And the directory is not consulted for it: its password does not work.
    let e = svc
        .login(req("admin@example.com", "ldap-pass", "10.0.0.2"))
        .await;
    assert!(matches!(e, Err(LoginError::InvalidCredentials)));
}
