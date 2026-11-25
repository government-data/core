//! Connects the system settings ([`SettingsService`]) with the real
//! adapters: Postgres to store them, AES-GCM for their secrets, and LDAP,
//! SMTP, Microsoft Graph and OIDC to apply and test them. It lives in the
//! composition root because `infrastructure` does not know `application`.

use std::sync::Arc;
use std::time::Duration;

use application::{
    DirectoryMember, DirectoryProbe, DirectoryProbeUser, LdapSettings, MailSettings, MailTransport,
    Mailer, OAuthProviderSettings, OAuthSettings, OidcDiscovery, OutgoingMail, SecretCipher,
    SettingsConnectors, SettingsStore, SmtpSecurity, StoredSection,
};
use async_trait::async_trait;
use auth_gateway::{DirectoryAuthenticator, OAuthProvider, ProviderId};
use infrastructure::{
    GraphConfig, GraphMailer, HttpClient, LdapAuthenticator, LdapConfig, OidcConfig, OidcProvider,
    OutboundMailer, PgSettingsStore, SecretBox, SmtpConfig, SmtpMailer, SmtpTls,
};

use crate::config::env_value;

/// [`SettingsStore`] over Postgres.
pub(crate) struct Store(pub(crate) PgSettingsStore);

fn section(row: infrastructure::SettingsRow) -> StoredSection {
    StoredSection {
        data: row.data,
        updated_at: row.updated_at,
        updated_by: row.updated_by,
    }
}

#[async_trait]
impl SettingsStore for Store {
    async fn load(&self, name: &str) -> Result<Option<StoredSection>, String> {
        Ok(self.0.load(name).await?.map(section))
    }

    async fn save(
        &self,
        name: &str,
        data: serde_json::Value,
        updated_by: &str,
    ) -> Result<StoredSection, String> {
        self.0.save(name, data, updated_by).await.map(section)
    }

    async fn delete(&self, name: &str) -> Result<(), String> {
        self.0.delete(name).await
    }
}

/// [`SecretCipher`] with AES-256-GCM.
pub(crate) struct Cipher(pub(crate) SecretBox);

impl SecretCipher for Cipher {
    fn encrypt(&self, plain: &str) -> Result<String, String> {
        self.0.encrypt(plain)
    }

    fn decrypt(&self, sealed: &str) -> Result<String, String> {
        self.0.decrypt(sealed)
    }
}

struct MailerAdapter(OutboundMailer);

#[async_trait]
impl Mailer for MailerAdapter {
    async fn send(&self, mail: &OutgoingMail) -> Result<(), String> {
        self.0
            .send(&mail.to, &mail.subject, &mail.text, mail.html.as_deref())
            .await
    }
}

/// The real adapters.
pub(crate) struct Connectors {
    pub(crate) http: Arc<dyn HttpClient>,
}

fn ldap_config(s: &LdapSettings) -> LdapConfig {
    LdapConfig {
        url: s.url.clone(),
        starttls: s.starttls,
        tls_insecure: s.tls_insecure,
        bind_dn: s.bind_dn.clone(),
        bind_password: s.bind_password.clone(),
        base_dn: s.base_dn.clone(),
        user_filter: s.user_filter.clone(),
        timeout: Duration::from_secs(s.timeout_secs.max(1)),
    }
}

#[async_trait]
impl SettingsConnectors for Connectors {
    fn directory(&self, s: &LdapSettings) -> Result<Arc<dyn DirectoryAuthenticator>, String> {
        Ok(Arc::new(LdapAuthenticator::new(ldap_config(s))?))
    }

    async fn probe_directory(
        &self,
        s: &LdapSettings,
        login: Option<&str>,
    ) -> Result<DirectoryProbe, String> {
        let probe = LdapAuthenticator::new(ldap_config(s))?.probe(login).await?;
        Ok(DirectoryProbe {
            user: probe.user.map(|u| DirectoryProbeUser {
                dn: u.dn,
                email: u.email,
                display_name: u.display_name,
                groups: u.groups,
            }),
        })
    }

    async fn directory_group_members(
        &self,
        s: &LdapSettings,
        group_dn: &str,
    ) -> Result<Vec<DirectoryMember>, String> {
        let people = LdapAuthenticator::new(ldap_config(s))?
            .group_members(group_dn)
            .await?;
        Ok(people
            .into_iter()
            .map(|p| DirectoryMember {
                email: p.email,
                display_name: p.display_name,
            })
            .collect())
    }

    fn oauth_provider(&self, s: &OAuthProviderSettings) -> Result<Arc<dyn OAuthProvider>, String> {
        let config = OidcConfig {
            provider_id: ProviderId::parse(&s.id).map_err(|e| e.to_string())?,
            client_id: s.client_id.clone(),
            client_secret: s.client_secret.clone(),
            auth_endpoint: s.auth_endpoint.clone(),
            token_endpoint: s.token_endpoint.clone(),
            userinfo_endpoint: s.userinfo_endpoint.clone(),
            issuer: s.issuer.clone(),
            scopes: s.scopes.clone(),
            trust_email: s.trust_email,
        };
        Ok(Arc::new(OidcProvider::new(config, self.http.clone())))
    }

    async fn discover_oidc(&self, issuer: &str) -> Result<OidcDiscovery, String> {
        let e = infrastructure::discover_oidc(issuer).await?;
        Ok(OidcDiscovery {
            issuer: e.issuer,
            auth_endpoint: e.auth_endpoint,
            token_endpoint: e.token_endpoint,
            userinfo_endpoint: e.userinfo_endpoint,
        })
    }

    fn mailer(&self, s: &MailSettings) -> Result<Arc<dyn Mailer>, String> {
        let mailer = match s.transport {
            MailTransport::Smtp => OutboundMailer::Smtp(SmtpMailer::new(SmtpConfig {
                host: s.smtp_host.clone(),
                port: s.smtp_port,
                tls: match s.smtp_security {
                    SmtpSecurity::None => SmtpTls::None,
                    SmtpSecurity::Starttls => SmtpTls::StartTls,
                    SmtpSecurity::Tls => SmtpTls::Implicit,
                },
                credentials: (!s.smtp_username.is_empty())
                    .then(|| (s.smtp_username.clone(), s.smtp_password.clone())),
                from_address: s.from_address.clone(),
                from_name: s.from_name.clone(),
            })?),
            MailTransport::Graph => OutboundMailer::Graph(GraphMailer::new(GraphConfig {
                tenant_id: s.graph_tenant_id.clone(),
                client_id: s.graph_client_id.clone(),
                client_secret: s.graph_client_secret.clone(),
                from_address: s.from_address.clone(),
                from_name: s.from_name.clone(),
            })),
        };
        Ok(Arc::new(MailerAdapter(mailer)))
    }
}

/// `LDAP_*` from the environment, as the LDAP section's initial value.
pub(crate) fn ldap_from_env() -> Result<Option<LdapSettings>, String> {
    Ok(LdapConfig::from_env()?.map(|c| LdapSettings {
        enabled: true,
        url: c.url,
        starttls: c.starttls,
        tls_insecure: c.tls_insecure,
        bind_dn: c.bind_dn,
        bind_password: c.bind_password,
        base_dn: c.base_dn,
        user_filter: c.user_filter,
        admin_groups: infrastructure::ldap_admin_groups_from_env(),
        timeout_secs: c.timeout.as_secs(),
    }))
}

/// `OAUTH_*` from the environment, as the OAuth section's initial value.
pub(crate) fn oauth_from_env(providers: &[OidcConfig]) -> Option<OAuthSettings> {
    (!providers.is_empty()).then(|| OAuthSettings {
        providers: providers
            .iter()
            .map(|p| OAuthProviderSettings {
                id: p.provider_id.as_str().to_string(),
                enabled: true,
                client_id: p.client_id.clone(),
                client_secret: p.client_secret.clone(),
                issuer: p.issuer.clone(),
                auth_endpoint: p.auth_endpoint.clone(),
                token_endpoint: p.token_endpoint.clone(),
                userinfo_endpoint: p.userinfo_endpoint.clone(),
                scopes: p.scopes.clone(),
                trust_email: p.trust_email,
            })
            .collect(),
    })
}

/// [`api::captcha::TokenVerifier`] with Turnstile.
pub(crate) struct TurnstileVerifier(pub(crate) infrastructure::turnstile::Turnstile);

#[async_trait]
impl api::captcha::TokenVerifier for TurnstileVerifier {
    async fn verify(&self, secret: &str, token: &str, ip: &str) -> Result<bool, String> {
        self.0.verify(secret, token, ip).await
    }
}

/// The captcha (Turnstile), with its keys from the environment
/// (`TURNSTILE_SITE_KEY` and `TURNSTILE_SECRET` or `_FILE`) when they are
/// there; without them, nothing is asked.
pub(crate) fn captcha_from_env() -> Result<api::captcha::Captcha, String> {
    let captcha = api::captcha::Captcha::new(Arc::new(TurnstileVerifier(
        infrastructure::turnstile::Turnstile::new()?,
    )));
    let secret = match env_value("TURNSTILE_SECRET_FILE") {
        Some(f) => Some(
            std::fs::read_to_string(&f)
                .map_err(|e| format!("TURNSTILE_SECRET_FILE {f}: {e}"))?
                .trim()
                .to_string(),
        ),
        None => env_value("TURNSTILE_SECRET"),
    };
    match (env_value("TURNSTILE_SITE_KEY"), secret) {
        (Some(site), Some(secret)) => {
            captcha.configure(Some((site, secret)));
            tracing::info!("captcha (Turnstile) from the environment");
            Ok(captcha)
        }
        _ => Ok(captcha),
    }
}
