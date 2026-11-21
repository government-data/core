//! The OAuth2/OIDC section, and the shared account (an instance we run signs
//! in with the account the management panel gives it).

use std::collections::{BTreeMap, HashSet};

use auth_gateway::ProviderId;

use super::{
    clean_list, invalid, OAuthProviderSettings, OAuthSettings, OidcDiscovery, Section,
    SettingsError, SettingsService, OAUTH,
};

impl SettingsService {
    pub(super) fn apply_oauth(&self, s: &OAuthSettings) -> Result<(), String> {
        let platform = self
            .platform_oauth
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        // With the shared account, only it: the management panel controls the
        // access and whoever signs in is tied to the account. The company's
        // own providers stay stored, unused.
        let providers = match platform.as_ref() {
            Some(p) => vec![self.connectors.oauth_provider(p)?],
            None => s
                .providers
                .iter()
                .filter(|p| p.enabled)
                .map(|p| self.connectors.oauth_provider(p))
                .collect::<Result<Vec<_>, _>>()?,
        };
        self.oauth.set_providers(providers);
        Ok(())
    }

    /// Sign in with the shared account: the provider the management panel
    /// gives an instance we run (`None`: none). It replaces the company's and
    /// applies at once.
    pub async fn set_platform_oauth(
        &self,
        p: Option<OAuthProviderSettings>,
    ) -> Result<(), SettingsError> {
        *self
            .platform_oauth
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = p;
        self.ensure_loaded().await?;
        self.apply_oauth(&self.read().oauth.value)
            .map_err(SettingsError::Internal)
    }

    /// The shared account this instance signs in with (`https://cuenta.…`,
    /// without a trailing `/`), to sign out through it or switch application.
    /// `None` without the account.
    pub fn platform_account(&self) -> Option<String> {
        self.platform_oauth
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|p| p.issuer.trim_end_matches('/').to_string())
    }

    /// The shared account's applications `email` can go to, for the menu:
    /// `None` without the account.
    pub async fn platform_apps(&self, email: &str) -> Option<Result<serde_json::Value, String>> {
        let p = self
            .platform_oauth
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()?;
        Some(self.connectors.account_apps(&p, email).await)
    }

    /// The callback URL to register with the provider.
    pub fn oauth_redirect_uri(&self, provider_id: &str) -> String {
        format!(
            "{}/auth/oauth/{provider_id}/callback",
            self.oauth.redirect_base().trim_end_matches('/')
        )
    }

    fn complete_oauth(&self, mut s: OAuthSettings) -> Result<OAuthSettings, SettingsError> {
        // Secrets are kept by provider id.
        let current = self.read().oauth.value.clone();
        let mut errors = BTreeMap::new();
        let mut seen = HashSet::new();
        for (i, p) in s.providers.iter_mut().enumerate() {
            let field = |name: &str| format!("providers.{i}.{name}");
            p.id = p.id.trim().to_ascii_lowercase();
            for v in [
                &mut p.client_id,
                &mut p.issuer,
                &mut p.auth_endpoint,
                &mut p.token_endpoint,
                &mut p.userinfo_endpoint,
            ] {
                *v = v.trim().to_string();
            }
            if p.client_secret.is_empty() {
                if let Some(old) = current.providers.iter().find(|o| o.id == p.id) {
                    p.client_secret = old.client_secret.clone();
                }
            }
            p.scopes = clean_list(std::mem::take(&mut p.scopes));
            if p.scopes.is_empty() {
                p.scopes = vec!["openid".into(), "email".into(), "profile".into()];
            }
            let url_safe = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
            if p.id.is_empty() || !p.id.chars().all(url_safe) || ProviderId::parse(&p.id).is_err() {
                errors.insert(field("id"), texts::tr!("app.settings.slug"));
            } else if !seen.insert(p.id.clone()) {
                errors.insert(field("id"), "Repetido".into());
            }
            if !p.enabled {
                continue;
            }
            if p.client_id.is_empty() {
                errors.insert(field("client_id"), "Requerido".into());
            }
            if p.client_secret.is_empty() {
                errors.insert(field("client_secret"), "Requerido".into());
            }
            for (name, url) in [
                ("issuer", &p.issuer),
                ("auth_endpoint", &p.auth_endpoint),
                ("token_endpoint", &p.token_endpoint),
                ("userinfo_endpoint", &p.userinfo_endpoint),
            ] {
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    errors.insert(field(name), "URL http(s) requerida".into());
                }
            }
        }
        if errors.is_empty() {
            Ok(s)
        } else {
            Err(SettingsError::Invalid(errors))
        }
    }

    pub async fn save_oauth(
        &self,
        input: OAuthSettings,
        actor: &str,
    ) -> Result<Section<OAuthSettings>, SettingsError> {
        if self.platform_account().is_some() {
            return Err(SettingsError::Managed);
        }
        let value = self.complete_oauth(input)?;
        for (i, p) in value.providers.iter().enumerate() {
            if p.enabled {
                self.connectors
                    .oauth_provider(p)
                    .map_err(|e| invalid(&format!("providers.{i}.id"), &e))?;
            }
        }
        let source = self.persist(OAUTH, &value, actor).await?;
        self.apply_oauth(&value).map_err(SettingsError::Internal)?;
        let section = Section { value, source };
        self.current.update(|c| c.oauth = section.clone());
        Ok(section)
    }

    /// Read an OIDC issuer's discovery document.
    pub async fn discover_oidc(&self, issuer: &str) -> Result<OidcDiscovery, SettingsError> {
        let issuer = issuer.trim().trim_end_matches('/');
        if !(issuer.starts_with("https://") || issuer.starts_with("http://")) {
            return Err(invalid("issuer", "URL http(s) requerida"));
        }
        self.connectors
            .discover_oidc(issuer)
            .await
            .map_err(SettingsError::Unreachable)
    }
}
