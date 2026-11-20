//! The local installation's license: it loads the last stored permit (and
//! checks it again), renews it with the cloud and answers which features and
//! limits hold now. Without a valid permit (or past the grace period) the
//! installation is on the basic plan.

use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use license_model::{
    contract_status, status_at, ContractStatus, Entitlement, FetchError, LicenseAuthority,
    LicenseStatus, LicenseStore, Limits,
};
use serde::Serialize;

use crate::settings::MailSlot;

mod reminders;
#[cfg(test)]
mod tests;

pub use reminders::{AdminContacts, PlatformAdmins};

/// How often the cloud is told the installation is still up (it takes as
/// down one that is silent for about 15 min).
pub const HEARTBEAT_EVERY: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// How often the permit is renewed, and how often it is retried if it fails.
pub const RENEW_EVERY: std::time::Duration = std::time::Duration::from_secs(24 * 3600);
pub const RETRY_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);

#[derive(Debug, Clone, Default)]
struct Snapshot {
    entitlement: Option<Entitlement>,
    key_hint: Option<String>,
    fetched_at: Option<DateTime<Utc>>,
    last_attempt_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
}

/// How the license stands, for the License screen.
#[derive(Debug, Clone, Serialize)]
pub struct LicenseView {
    pub status: LicenseStatus,
    pub entitlement: Option<Entitlement>,
    /// The limits that hold now (the plan's, or the basic ones).
    pub limits: Limits,
    /// Where the contract stands (with a valid permit).
    pub contract: Option<ContractStatus>,
    pub installation: String,
    /// The key's first characters, to recognize it.
    pub key_hint: Option<String>,
    pub fetched_at: Option<DateTime<Utc>>,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    /// The binary trusts the development key: it is not for customers.
    pub dev_key: bool,
}

/// The license service.
#[derive(Clone)]
pub struct LicenseService {
    authority: Arc<dyn LicenseAuthority>,
    store: Arc<dyn LicenseStore>,
    installation: String,
    /// The key it was installed with (`ORQ_LICENSE_KEY`); the UI's rules if
    /// there is one.
    install_key: Option<String>,
    dev_key: bool,
    state: Arc<RwLock<Snapshot>>,
    /// Where the month's usage reported on renewal comes from.
    usage: Option<Arc<dyn platform::PlatformRepository>>,
    /// How, and to whom, to send the expiry notices.
    reminders: Option<(MailSlot, Arc<dyn AdminContacts>)>,
    /// The last billing the cloud gave, in case it does not answer later.
    billing: Arc<RwLock<Option<FetchedBilling>>>,
}

type FetchedBilling = (license_model::Billing, DateTime<Utc>);

/// The billing for the License screen: the cloud's or, if it does not answer,
/// the last one it gave (with the error).
#[derive(Debug, Clone, Serialize)]
pub struct BillingView {
    pub billing: Option<license_model::Billing>,
    pub fetched_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

impl std::fmt::Debug for LicenseService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LicenseService")
            .field("installation", &self.installation)
            .finish_non_exhaustive()
    }
}

/// The first day of the current month.
fn month_start() -> DateTime<Utc> {
    use chrono::{Datelike, TimeZone};
    let now = Utc::now();
    Utc.with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
        .single()
        .unwrap_or(now)
}

fn hint(key: &str) -> String {
    key.chars().take(8).collect::<String>() + "…"
}

impl LicenseService {
    pub fn new(
        authority: Arc<dyn LicenseAuthority>,
        store: Arc<dyn LicenseStore>,
        installation: String,
        install_key: Option<String>,
        dev_key: bool,
    ) -> Self {
        Self {
            authority,
            store,
            installation,
            install_key: install_key.filter(|k| !k.trim().is_empty()),
            dev_key,
            state: Arc::new(RwLock::new(Snapshot::default())),
            usage: None,
            reminders: None,
            billing: Arc::default(),
        }
    }

    /// Report the month's usage (executions) on each renewal, so the cloud
    /// sees it when quoting the renewal.
    pub fn with_usage(mut self, repo: Arc<dyn platform::PlatformRepository>) -> Self {
        self.usage = Some(repo);
        self
    }

    /// The key in use: the one changed from the UI, or the installation's.
    async fn key(&self) -> Result<Option<String>, String> {
        Ok(self
            .store
            .load()
            .await?
            .key
            .or_else(|| self.install_key.clone()))
    }

    /// This installation's payments and renewal, asked to the cloud.
    pub async fn billing(&self) -> Result<BillingView, String> {
        let Some(key) = self.key().await? else {
            return Err(texts::tr!("app.licensing.noActivationKey"));
        };
        let fetched = self.authority.billing(&key, &self.installation).await;
        let mut cache = self.billing.write().map_err(|e| e.to_string())?;
        Ok(match fetched {
            Ok(billing) => {
                let now = Utc::now();
                *cache = Some((billing.clone(), now));
                BillingView {
                    billing: Some(billing),
                    fetched_at: Some(now),
                    error: None,
                }
            }
            Err(e) => BillingView {
                billing: cache.as_ref().map(|(b, _)| b.clone()),
                fetched_at: cache.as_ref().map(|(_, at)| *at),
                error: Some(e),
            },
        })
    }

    /// On start-up: the last stored permit, checked again (someone may have
    /// edited it in the database).
    pub async fn load(&self) -> Result<(), String> {
        let stored = self.store.load().await?;
        let key = stored.key.clone().or_else(|| self.install_key.clone());
        let entitlement = match &stored.permit {
            Some(permit) => match self.check(permit) {
                Ok(e) => Some(e),
                Err(e) => {
                    tracing::warn!("the stored permit is not valid: {e}");
                    None
                }
            },
            None => None,
        };
        let mut state = self.state.write().map_err(|e| e.to_string())?;
        *state = Snapshot {
            entitlement,
            key_hint: key.as_deref().map(hint),
            fetched_at: stored.fetched_at,
            last_attempt_at: stored.last_attempt_at,
            last_error: stored.last_error,
        };
        Ok(())
    }

    /// Check the signature and that the permit belongs to this installation.
    fn check(&self, permit: &str) -> Result<Entitlement, String> {
        let entitlement = self.authority.verify(permit)?;
        if entitlement.installation != self.installation {
            return Err(texts::tr!("app.licensing.permitOfOtherInstallation"));
        }
        Ok(entitlement)
    }

    /// Tell the cloud the installation is still up, with the administrators'
    /// emails so it writes to them if it goes silent. Without a key there is
    /// nothing to tell.
    pub async fn heartbeat(&self) -> Result<(), String> {
        let Some(key) = self.key().await? else {
            return Ok(());
        };
        let notify = match &self.reminders {
            Some((_, admins)) => admins.admin_emails().await?,
            None => Vec::new(),
        };
        self.authority
            .heartbeat(&key, &self.installation, &notify)
            .await
    }

    /// Ask the cloud for the current permit. If it fails, the last good one
    /// is kept (the grace period runs from its expiry).
    pub async fn refresh(&self) -> Result<(), String> {
        let Some(key) = self.key().await? else {
            return Err(texts::tr!("app.licensing.noActivationKey"));
        };
        let usage = match &self.usage {
            Some(repo) => repo.usage(month_start()).await.ok(),
            None => None,
        };
        let outcome = async {
            let permit = self
                .authority
                .fetch(&key, &self.installation, usage.as_ref())
                .await?;
            let entitlement = self.check(&permit).map_err(FetchError::Unreachable)?;
            self.store
                .save_permit(&permit)
                .await
                .map_err(FetchError::Unreachable)?;
            Ok::<_, FetchError>(entitlement)
        }
        .await;
        let now = Utc::now();
        let error = {
            let mut state = self.state.write().map_err(|e| e.to_string())?;
            state.key_hint = Some(hint(&key));
            state.last_attempt_at = Some(now);
            match outcome {
                Ok(entitlement) => {
                    state.entitlement = Some(entitlement);
                    state.fetched_at = Some(now);
                    state.last_error = None;
                    None
                }
                Err(e) => {
                    state.last_error = Some(e.to_string());
                    // The cloud said no: the permit is lost at once, no grace.
                    if matches!(e, FetchError::Rejected(_)) {
                        state.entitlement = None;
                    }
                    Some(e)
                }
            }
        };
        match error {
            None => Ok(()),
            Some(e) => {
                let text = e.to_string();
                let _ = match e {
                    FetchError::Rejected(_) => self.store.clear_permit(&text).await,
                    FetchError::Unreachable(_) => self.store.record_error(&text).await,
                };
                Err(text)
            }
        }
    }

    /// Change the key (from the UI) and activate it at once.
    pub async fn set_key(&self, key: &str) -> Result<(), String> {
        let key = key.trim();
        if key.is_empty() {
            return Err(texts::tr!("app.licensing.emptyKey"));
        }
        self.store.save_key(key).await?;
        if let Ok(mut state) = self.state.write() {
            state.entitlement = None;
        }
        self.refresh().await
    }

    fn snapshot(&self) -> Snapshot {
        self.state.read().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn status(&self) -> LicenseStatus {
        let snapshot = self.snapshot();
        match status_at(snapshot.entitlement.as_ref(), Utc::now()) {
            // With a key but no valid permit: it is not that the key is
            // missing.
            LicenseStatus::Basic { .. }
                if snapshot.entitlement.is_none() && snapshot.key_hint.is_some() =>
            {
                LicenseStatus::Basic {
                    reason: match snapshot.last_error {
                        Some(e) => texts::tr!("app.licensing.invalidPermit", e = e),
                        None => texts::tr!("app.licensing.noPermitYet"),
                    },
                }
            }
            status => status,
        }
    }

    /// The permit that holds now (current or in grace).
    fn valid(&self) -> Option<Entitlement> {
        let snapshot = self.snapshot();
        match status_at(snapshot.entitlement.as_ref(), Utc::now()) {
            LicenseStatus::Basic { .. } => None,
            _ => snapshot.entitlement,
        }
    }

    /// Where the contract stands, with a valid permit.
    pub fn contract(&self) -> Option<ContractStatus> {
        self.valid().map(|e| contract_status(&e, Utc::now()))
    }

    /// Why the installation is read-only, if it is.
    pub fn read_only(&self) -> Option<String> {
        match self.contract() {
            Some(ContractStatus::ReadOnly { reason }) => Some(reason),
            _ => None,
        }
    }

    pub fn limits(&self) -> Limits {
        self.valid().map(|e| e.limits).unwrap_or(Limits::BASIC)
    }

    pub fn has_feature(&self, feature: &str) -> bool {
        self.valid().is_some_and(|e| e.has(feature))
    }

    pub fn view(&self) -> LicenseView {
        let snapshot = self.snapshot();
        LicenseView {
            status: self.status(),
            limits: self.limits(),
            contract: self.contract(),
            entitlement: snapshot.entitlement,
            installation: self.installation.clone(),
            key_hint: snapshot.key_hint,
            fetched_at: snapshot.fetched_at,
            last_attempt_at: snapshot.last_attempt_at,
            last_error: snapshot.last_error,
            dev_key: self.dev_key,
        }
    }
}
