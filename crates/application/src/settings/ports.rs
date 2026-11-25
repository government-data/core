//! The ports the settings service needs: where sections are stored, how their
//! secrets are encrypted, and the adapters each section builds.

use std::sync::Arc;

use async_trait::async_trait;
use auth_gateway::{DirectoryAuthenticator, Mailer, OAuthProvider};
use chrono::{DateTime, Utc};

use super::{LdapSettings, MailSettings, OAuthProviderSettings};

/// A section as it was stored.
#[derive(Debug, Clone)]
pub struct StoredSection {
    /// The document, with its secrets already encrypted.
    pub data: serde_json::Value,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<String>,
}

/// Where the sections are stored.
#[async_trait]
pub trait SettingsStore: Send + Sync {
    async fn load(&self, section: &str) -> Result<Option<StoredSection>, String>;
    async fn save(
        &self,
        section: &str,
        data: serde_json::Value,
        updated_by: &str,
    ) -> Result<StoredSection, String>;
    async fn delete(&self, section: &str) -> Result<(), String>;
}

/// Encrypts and decrypts the secrets that are stored.
pub trait SecretCipher: Send + Sync {
    fn encrypt(&self, plain: &str) -> Result<String, String>;
    fn decrypt(&self, sealed: &str) -> Result<String, String>;
}

/// What a directory test found.
#[derive(Debug, Clone, Default)]
pub struct DirectoryProbe {
    /// The person looked up, if one was asked for and found.
    pub user: Option<DirectoryProbeUser>,
}

#[derive(Debug, Clone)]
pub struct DirectoryProbeUser {
    pub dn: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub groups: Vec<String>,
}

/// A person who is in a directory group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryMember {
    /// In lower case.
    pub email: String,
    pub display_name: Option<String>,
}

/// The endpoints an OIDC issuer publishes in its discovery document.
#[derive(Debug, Clone)]
pub struct OidcDiscovery {
    pub issuer: String,
    pub auth_endpoint: String,
    pub token_endpoint: String,
    pub userinfo_endpoint: String,
}

/// Builds the real adapters from a section, and tests them.
#[async_trait]
pub trait SettingsConnectors: Send + Sync {
    fn directory(&self, s: &LdapSettings) -> Result<Arc<dyn DirectoryAuthenticator>, String>;
    /// Connect with the service account and, given a `login`, look that
    /// person up (without checking any password).
    async fn probe_directory(
        &self,
        s: &LdapSettings,
        login: Option<&str>,
    ) -> Result<DirectoryProbe, String>;
    /// The people directly in the `group_dn` group.
    async fn directory_group_members(
        &self,
        _s: &LdapSettings,
        _group_dn: &str,
    ) -> Result<Vec<DirectoryMember>, String> {
        Err(texts::tr!("app.settings.directoryListsNoMembers"))
    }
    fn oauth_provider(&self, s: &OAuthProviderSettings) -> Result<Arc<dyn OAuthProvider>, String>;
    async fn discover_oidc(&self, issuer: &str) -> Result<OidcDiscovery, String>;
    fn mailer(&self, s: &MailSettings) -> Result<Arc<dyn Mailer>, String>;
}
