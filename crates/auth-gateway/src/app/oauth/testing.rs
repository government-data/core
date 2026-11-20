//! In-memory doubles of the OAuth ports, for tests here and in other crates
//! (the `testing` feature).

use super::*;
use std::sync::Mutex;
use std::time::SystemTime;

use crate::{AdminAccount, LockoutState, NewAdmin, PkcePair};
use domain::RepositoryError;

#[derive(Debug, Default)]
pub struct InMemoryPending {
    pub rows: Mutex<HashMap<String, PendingAuthorization>>,
}
#[async_trait::async_trait]
impl PendingAuthStore for InMemoryPending {
    async fn insert(&self, pending: &PendingAuthorization) -> Result<(), RepositoryError> {
        self.rows
            .lock()
            .unwrap()
            .insert(pending.state.clone(), pending.clone());
        Ok(())
    }
    async fn consume(&self, state: &str) -> Result<Option<PendingAuthorization>, RepositoryError> {
        Ok(self.rows.lock().unwrap().remove(state))
    }
}

#[derive(Default)]
pub(crate) struct InMemoryIdentities {
    pub links: Mutex<HashMap<(String, String), AdminId>>,
}
#[async_trait::async_trait]
impl OAuthIdentityRepository for InMemoryIdentities {
    async fn find_admin(
        &self,
        provider: &ProviderId,
        subject: &str,
    ) -> Result<Option<AdminId>, RepositoryError> {
        Ok(self
            .links
            .lock()
            .unwrap()
            .get(&(provider.as_str().to_string(), subject.to_string()))
            .cloned())
    }
    async fn link(
        &self,
        provider: &ProviderId,
        subject: &str,
        _email: Option<&str>,
        admin_id: &AdminId,
    ) -> Result<(), RepositoryError> {
        self.links
            .lock()
            .unwrap()
            .entry((provider.as_str().to_string(), subject.to_string()))
            .or_insert_with(|| admin_id.clone());
        Ok(())
    }
    async fn list_for_admin(
        &self,
        admin_id: &AdminId,
    ) -> Result<Vec<LinkedIdentity>, RepositoryError> {
        let mut out: Vec<LinkedIdentity> = self
            .links
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, a)| *a == admin_id)
            .map(|((provider, subject), _)| LinkedIdentity {
                provider: ProviderId::parse(provider).unwrap(),
                subject: subject.clone(),
                email: None,
                linked_at: SystemTime::UNIX_EPOCH,
            })
            .collect();
        out.sort_by(|a, b| a.subject.cmp(&b.subject));
        Ok(out)
    }
    async fn unlink(
        &self,
        admin_id: &AdminId,
        provider: &ProviderId,
        subject: &str,
    ) -> Result<bool, RepositoryError> {
        let mut links = self.links.lock().unwrap();
        let key = (provider.as_str().to_string(), subject.to_string());
        if links.get(&key) == Some(admin_id) {
            links.remove(&key);
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[derive(Debug, Default)]
pub struct InMemoryAdmins {
    pub accounts: Mutex<Vec<AdminAccount>>,
}
#[async_trait::async_trait]
impl AdminRepository for InMemoryAdmins {
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
        _id: &AdminId,
        _state: &LockoutState,
    ) -> Result<(), RepositoryError> {
        Ok(())
    }
    async fn count(&self) -> Result<u64, RepositoryError> {
        Ok(self.accounts.lock().unwrap().len() as u64)
    }
    async fn insert(&self, _admin: &NewAdmin) -> Result<AdminId, RepositoryError> {
        unimplemented!("not exercised by the oauth flow")
    }
}

#[derive(Debug)]
pub struct FixedSecrets;
impl OAuthSecretGenerator for FixedSecrets {
    fn state(&self) -> String {
        "state-123".to_string()
    }
    fn nonce(&self) -> String {
        "nonce-abc".to_string()
    }
    fn pkce(&self) -> PkcePair {
        PkcePair::new("verifier-xyz", "challenge-xyz")
    }
    fn challenge(&self, verifier: &str) -> String {
        format!("challenge-of-{verifier}")
    }
}

#[derive(Debug)]
pub struct FixedClock(pub SystemTime);
impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

/// A service with no providers, for other crates' tests.
pub fn empty_service() -> OAuthLoginService {
    OAuthLoginService::new(
        Vec::new(),
        Arc::new(InMemoryPending::default()),
        Arc::new(InMemoryIdentities::default()),
        Arc::new(InMemoryAdmins::default()),
        Arc::new(FixedSecrets),
        Arc::new(FixedClock(SystemTime::UNIX_EPOCH)),
        "https://admin.example.com",
    )
}
