//! System settings editable from the UI: the LDAP directory, outgoing mail
//! (SMTP, or Microsoft Graph for Office 365) and OAuth2/OIDC sign-in
//! providers.
//!
//! Each section is stored as a JSON document with its secrets encrypted
//! ([`SecretCipher`]); saving it applies it live, without a restart. A section
//! that was never saved takes what the environment variables bring (`LDAP_*`,
//! `OAUTH_*`), so an existing installation stays the same until someone edits
//! it here.
//!
//! Secrets never leave this service towards the client: an empty secret in
//! what is sent means "keep the current one".

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use auth_gateway::{DirectoryAccounts, DirectorySlot, OAuthLoginService};
use chrono::{DateTime, Utc};
use serde::Serialize;

pub use auth_gateway::{MailNotice, MailSlot, MailTone, Mailer, OutgoingMail};

mod ldap;
mod mail;
mod oauth;
mod ports;
mod sections;
#[cfg(test)]
mod tests;

pub use ports::{
    DirectoryMember, DirectoryProbe, DirectoryProbeUser, OidcDiscovery, SecretCipher,
    SettingsConnectors, SettingsStore, StoredSection,
};
use sections::Secrets;
pub use sections::{
    entra_own_tenant, LdapSettings, MailSettings, MailTransport, OAuthProviderSettings,
    OAuthSettings, SmtpSecurity, DEFAULT_LDAP_USER_FILTER,
};

/// Where a section's settings in use come from.
#[derive(Debug, Clone, Default)]
pub enum SettingsSource {
    /// Saved from the UI.
    Database {
        updated_at: DateTime<Utc>,
        updated_by: Option<String>,
    },
    /// Taken from the environment variables at start-up.
    Environment,
    /// Not configured.
    #[default]
    Unset,
}

/// A section in use and where it comes from. It carries the secrets: the HTTP
/// layer replaces them with "there is / there is not" before answering.
#[derive(Debug, Clone, Default)]
pub struct Section<T> {
    pub value: T,
    pub source: SettingsSource,
}

/// What a settings operation that went wrong answered.
#[derive(Debug)]
pub enum SettingsError {
    /// Invalid fields: field → reason.
    Invalid(BTreeMap<String, String>),
    /// The test against the external service failed; the reason is for the
    /// admin.
    Unreachable(String),
    /// The management panel (the cloud) runs it: the access, with the shared
    /// account, or the platform's mail. It is not changed here.
    Managed,
    Internal(String),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(fields) => write!(f, "configuración inválida: {fields:?}"),
            Self::Unreachable(e) => write!(f, "{e}"),
            Self::Managed => write!(f, "lo administra gestión"),
            Self::Internal(e) => write!(f, "error interno: {e}"),
        }
    }
}

impl std::error::Error for SettingsError {}

fn invalid(field: &str, message: &str) -> SettingsError {
    SettingsError::Invalid(BTreeMap::from([(field.to_string(), message.to_string())]))
}

/// The lifetime that can be chosen for recovery links: from 5 minutes to a
/// day.
pub const MIN_RESET_LINK_MINUTES: u64 = 5;
pub const MAX_RESET_LINK_MINUTES: u64 = 24 * 60;

const LDAP: &str = "ldap";
const MAIL: &str = "mail";
const OAUTH: &str = "oauth";

/// What the environment variables bring, for the sections never saved.
#[derive(Debug, Clone, Default)]
pub struct EnvSettings {
    pub ldap: Option<LdapSettings>,
    pub oauth: Option<OAuthSettings>,
    /// `PASSWORD_RESET_TTL_MINUTES`: the links' lifetime while the mail sets
    /// no other (shown in the UI as the default).
    pub reset_link_minutes: u64,
}

/// What is applied for a company (in memory per tenant).
#[derive(Debug, Clone, Default)]
struct Current {
    ldap: Section<LdapSettings>,
    mail: Section<MailSettings>,
    oauth: Section<OAuthSettings>,
}

/// Reads, validates, saves, tests and applies the system settings.
#[derive(Clone)]
pub struct SettingsService {
    store: Arc<dyn SettingsStore>,
    cipher: Arc<dyn SecretCipher>,
    connectors: Arc<dyn SettingsConnectors>,
    directory: DirectorySlot,
    directory_accounts: Arc<dyn DirectoryAccounts>,
    oauth: OAuthLoginService,
    mail: MailSlot,
    env: EnvSettings,
    key_source: &'static str,
    current: tenancy::PerTenant<Current>,
    /// The platform's mail (online, the one the management panel sends), if
    /// any.
    platform: Arc<RwLock<Option<MailSettings>>>,
    /// Sign-in with the shared account (an instance we run: the provider the
    /// management panel gives it), if any. It goes in addition to the ones
    /// the company configures.
    platform_oauth: Arc<RwLock<Option<OAuthProviderSettings>>>,
}

impl std::fmt::Debug for SettingsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsService").finish_non_exhaustive()
    }
}

/// What the service applies the settings to.
#[derive(Clone)]
pub struct SettingsTargets {
    /// The [`auth_gateway::LoginService`]'s directory.
    pub directory: DirectorySlot,
    /// The local accounts of the directory's users.
    pub directory_accounts: Arc<dyn DirectoryAccounts>,
    /// The OAuth service whose providers are replaced.
    pub oauth: OAuthLoginService,
}

impl std::fmt::Debug for SettingsTargets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsTargets").finish_non_exhaustive()
    }
}

impl SettingsService {
    pub fn new(
        store: Arc<dyn SettingsStore>,
        cipher: Arc<dyn SecretCipher>,
        connectors: Arc<dyn SettingsConnectors>,
        targets: SettingsTargets,
        env: EnvSettings,
    ) -> Self {
        Self {
            store,
            cipher,
            connectors,
            directory: targets.directory,
            directory_accounts: targets.directory_accounts,
            oauth: targets.oauth,
            mail: MailSlot::default(),
            env,
            key_source: "file",
            current: tenancy::PerTenant::default(),
            platform: Arc::default(),
            platform_oauth: Arc::default(),
        }
    }

    /// Where the key that encrypts the secrets comes from (`environment`,
    /// `file`, `generated`), to warn in the UI if it has to be backed up.
    #[must_use]
    pub fn with_key_source(mut self, source: &'static str) -> Self {
        self.key_source = source;
        self
    }

    pub fn key_source(&self) -> &'static str {
        self.key_source
    }

    /// Load the three sections (the database or, if never saved, the
    /// environment) and apply them. A saved section that can no longer be
    /// applied (a secret that does not decrypt with the current key, for
    /// example) is logged and left off: it does not stop the start-up.
    pub async fn init(&self) -> Result<(), SettingsError> {
        let ldap = self
            .load_section::<LdapSettings>(LDAP, self.env.ldap.clone())
            .await?;
        if let Err(e) = self.apply_ldap(&ldap.value) {
            tracing::error!("LDAP could not be applied, it stays off: {e}");
            self.directory.set(None);
        }
        let mail = self.load_section::<MailSettings>(MAIL, None).await?;
        if let Err(e) = self.apply_mail(&mail.value) {
            tracing::error!("mail could not be applied, it stays off: {e}");
            self.mail.set(None);
        }
        let oauth = self
            .load_section::<OAuthSettings>(OAUTH, self.env.oauth.clone())
            .await?;
        if let Err(e) = self.apply_oauth(&oauth.value) {
            tracing::error!("OAuth could not be applied, no providers: {e}");
            self.oauth.set_providers(Vec::new());
        }
        self.current.set(Current { ldap, mail, oauth });
        if let Err(e) = self.seed_loaded().await {
            tracing::error!("the platform mail could not be set for this company: {e}");
        }
        Ok(())
    }

    async fn load_section<T>(
        &self,
        section: &str,
        env: Option<T>,
    ) -> Result<Section<T>, SettingsError>
    where
        T: Secrets + Default + serde::de::DeserializeOwned,
    {
        let stored = self
            .store
            .load(section)
            .await
            .map_err(SettingsError::Internal)?;
        let Some(stored) = stored else {
            return Ok(match env {
                Some(value) => Section {
                    value,
                    source: SettingsSource::Environment,
                },
                None => Section {
                    value: T::default(),
                    source: SettingsSource::Unset,
                },
            });
        };
        let mut value: T = serde_json::from_value(stored.data)
            .map_err(|e| SettingsError::Internal(format!("{section}: {e}")))?;
        for secret in value.secrets_mut() {
            if !secret.is_empty() {
                match self.cipher.decrypt(secret) {
                    Ok(plain) => *secret = plain,
                    Err(e) => {
                        tracing::error!(section, "a stored secret cannot be decrypted: {e}");
                        secret.clear();
                    }
                }
            }
        }
        Ok(Section {
            value,
            source: SettingsSource::Database {
                updated_at: stored.updated_at,
                updated_by: stored.updated_by,
            },
        })
    }

    /// What is applied for the current company.
    fn read(&self) -> Current {
        self.current.get().unwrap_or_default()
    }

    /// Load the current company's settings if they are not in memory yet
    /// (online, each one its own; the first time they are needed).
    pub async fn ensure_loaded(&self) -> Result<(), SettingsError> {
        if self.current.contains() {
            return Ok(());
        }
        self.init().await
    }

    pub fn ldap(&self) -> Section<LdapSettings> {
        self.read().ldap.clone()
    }

    pub fn mail(&self) -> Section<MailSettings> {
        self.read().mail.clone()
    }

    pub fn oauth(&self) -> Section<OAuthSettings> {
        self.read().oauth.clone()
    }

    async fn persist<T: Secrets + Serialize + Clone>(
        &self,
        section: &str,
        value: &T,
        actor: &str,
    ) -> Result<SettingsSource, SettingsError> {
        let mut sealed = value.clone();
        for secret in sealed.secrets_mut() {
            if !secret.is_empty() {
                *secret = self
                    .cipher
                    .encrypt(secret)
                    .map_err(SettingsError::Internal)?;
            }
        }
        let data =
            serde_json::to_value(&sealed).map_err(|e| SettingsError::Internal(e.to_string()))?;
        let stored = self
            .store
            .save(section, data, actor)
            .await
            .map_err(SettingsError::Internal)?;
        tracing::info!(section, actor, "settings saved");
        Ok(SettingsSource::Database {
            updated_at: stored.updated_at,
            updated_by: stored.updated_by,
        })
    }

    /// Go back to what the environment variables say.
    pub async fn reset(&self, section: &str, actor: &str) -> Result<(), SettingsError> {
        if ![LDAP, MAIL, OAUTH].contains(&section) {
            return Err(invalid(
                "section",
                &texts::tr!("app.settings.unknownSection"),
            ));
        }
        if (section == OAUTH && self.platform_account().is_some())
            || (section == MAIL && self.platform_mail().is_some())
        {
            return Err(SettingsError::Managed);
        }
        self.store
            .delete(section)
            .await
            .map_err(SettingsError::Internal)?;
        tracing::info!(section, actor, "settings reset to the environment");
        match section {
            LDAP => {
                let s = self.load_section(LDAP, self.env.ldap.clone()).await?;
                if let Err(e) = self.apply_ldap(&s.value) {
                    tracing::error!("LDAP from the environment could not be applied: {e}");
                    self.directory.set(None);
                }
                self.current.update(|c| c.ldap = s);
            }
            MAIL => {
                let s = self.load_section(MAIL, None).await?;
                self.mail.set(None);
                self.mail.set_reset_link_minutes(0);
                self.mail.set_notice_language(texts::Language::default());
                self.current.update(|c| c.mail = s);
            }
            _ => {
                let s = self.load_section(OAUTH, self.env.oauth.clone()).await?;
                if let Err(e) = self.apply_oauth(&s.value) {
                    tracing::error!("OAuth from the environment could not be applied: {e}");
                    self.oauth.set_providers(Vec::new());
                }
                self.current.update(|c| c.oauth = s);
            }
        }
        Ok(())
    }
}

/// Copy `current`'s secrets into the ones that come empty in `value`.
fn keep_secrets<T: Secrets + Clone>(value: &mut T, current: &T) {
    let mut current = current.clone();
    for (new, old) in value.secrets_mut().into_iter().zip(current.secrets_mut()) {
        if new.is_empty() {
            *new = std::mem::take(old);
        }
    }
}

fn clean_list(items: Vec<String>) -> Vec<String> {
    items
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
