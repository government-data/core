//! Server configuration, sourced entirely from the environment.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use api::oauth::OAuthRedirects;
use auth_gateway::ProviderId;
use infrastructure::{OidcConfig, StripeConfig};

/// Runtime configuration for the HTTP server.
#[derive(Debug, Clone)]
pub(crate) struct Config {
    host: IpAddr,
    port: u16,
    cors_allowed_origins: Vec<String>,
}

impl Config {
    /// Read configuration from the environment.
    ///
    /// - `APP_HOST` — bind address (default `0.0.0.0`)
    /// - `APP_PORT` — bind port (default `8080`)
    /// - `CORS_ALLOWED_ORIGINS` — comma-separated exact origins allowed to make
    ///   credentialed cross-origin requests (e.g.
    ///   `https://admin.example.com,http://localhost:5173`). Unset or empty
    ///   means no origin is allowed — there is no wildcard fallback; a
    ///   production deployment behind a single origin (web + API same host,
    ///   the default Traefik routing) needs nothing here at all.
    pub(crate) fn from_env() -> Result<Self, ConfigError> {
        let host = match std::env::var("APP_HOST") {
            Ok(raw) => raw.parse().map_err(|_| ConfigError::InvalidHost(raw))?,
            Err(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        };
        let port = match std::env::var("APP_PORT") {
            Ok(raw) => raw.parse().map_err(|_| ConfigError::InvalidPort(raw))?,
            Err(_) => 8080,
        };
        let cors_allowed_origins = std::env::var("CORS_ALLOWED_ORIGINS")
            .ok()
            .map(|raw| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|origin| !origin.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            host,
            port,
            cors_allowed_origins,
        })
    }

    /// The address the HTTP server should bind to.
    pub(crate) fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.port)
    }

    /// The explicit CORS origin allowlist (possibly empty — never a wildcard).
    pub(crate) fn cors_allowed_origins(&self) -> &[String] {
        &self.cors_allowed_origins
    }
}

/// A malformed environment value.
#[derive(Debug)]
pub(crate) enum ConfigError {
    InvalidHost(String),
    InvalidPort(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidHost(v) => write!(f, "invalid APP_HOST: {v:?}"),
            Self::InvalidPort(v) => write!(f, "invalid APP_PORT: {v:?}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// OAuth/OIDC providers from the environment: the initial value of the OAuth
/// section of the system settings, used until an admin saves it in the UI.
#[derive(Debug, Clone)]
pub(crate) struct OAuthSettings {
    /// One [`OidcConfig`] per enabled provider.
    pub(crate) providers: Vec<OidcConfig>,
}

impl OAuthSettings {
    /// Read OAuth configuration from the environment.
    ///
    /// `OAUTH_PROVIDERS` is a comma-separated list of provider ids to enable
    /// (e.g. `google`). For each id `X`, the per-provider settings are read
    /// from `OAUTH_<X>_*` (uppercased):
    ///
    /// - `OAUTH_<X>_CLIENT_ID` / `_CLIENT_SECRET` (required)
    /// - `OAUTH_<X>_AUTH_ENDPOINT` / `_TOKEN_ENDPOINT` / `_USERINFO_ENDPOINT` /
    ///   `_ISSUER` (required)
    /// - `OAUTH_<X>_SCOPES` (comma-separated, default `openid,email`)
    ///
    /// The redirect settings are read by [`Self::redirects_from_env`].
    ///
    /// Returns `None` when `OAUTH_PROVIDERS` is unset or empty. A configured
    /// provider missing a required setting is an error, never a silent skip.
    pub(crate) fn from_env() -> Result<Option<Self>, OAuthConfigError> {
        let raw = std::env::var("OAUTH_PROVIDERS").unwrap_or_default();
        let ids: Vec<&str> = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if ids.is_empty() {
            return Ok(None);
        }

        let mut providers = Vec::with_capacity(ids.len());
        for id in ids {
            let provider_id = ProviderId::parse(id)
                .map_err(|e| OAuthConfigError(format!("invalid provider id {id:?}: {e}")))?;
            let key = |suffix: &str| format!("OAUTH_{}_{suffix}", id.to_ascii_uppercase());
            let required = |suffix: &str| -> Result<String, OAuthConfigError> {
                std::env::var(key(suffix))
                    .ok()
                    .filter(|v| !v.trim().is_empty())
                    .ok_or_else(|| OAuthConfigError(format!("{} is required", key(suffix))))
            };
            let scopes = std::env::var(key("SCOPES"))
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "openid,email".to_string())
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            let issuer = required("ISSUER")?;
            providers.push(OidcConfig {
                provider_id,
                client_id: required("CLIENT_ID")?,
                client_secret: required("CLIENT_SECRET")?,
                auth_endpoint: required("AUTH_ENDPOINT")?,
                token_endpoint: required("TOKEN_ENDPOINT")?,
                userinfo_endpoint: required("USERINFO_ENDPOINT")?,
                // Changed in Settings → OAuth; at first, as when adding
                // Microsoft there: trust only with a tenant of one's own.
                trust_email: application::entra_own_tenant(&issuer),
                issuer,
                scopes,
            });
        }

        Ok(Some(Self { providers }))
    }
}

impl OAuthSettings {
    /// `OAUTH_REDIRECT_BASE`, `OAUTH_SUCCESS_REDIRECT` and
    /// `OAUTH_FAILURE_REDIRECT`, which stay deployment settings even when the
    /// providers themselves are edited in the admin UI.
    pub(crate) fn redirects_from_env() -> (String, OAuthRedirects) {
        let var = |name: &str, default: &str| {
            std::env::var(name)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| default.to_string())
        };
        (
            var("OAUTH_REDIRECT_BASE", "http://localhost:8080"),
            OAuthRedirects {
                success: var("OAUTH_SUCCESS_REDIRECT", "/"),
                failure: var("OAUTH_FAILURE_REDIRECT", "/login"),
                ..OAuthRedirects::default()
            },
        )
    }
}

/// An environment variable's trimmed value; `None` when it is unset or blank.
pub(crate) fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// The public address of the web app, for the links in password-reset
/// emails (on-premise): `APP_PUBLIC_URL`, else `OAUTH_REDIRECT_BASE` without
/// its trailing `/api`, else `http://localhost:8080`. Never the request's
/// `Host`, which a client could forge to send someone a link to its own site.
pub(crate) fn public_url_from_env() -> String {
    let var = |name: &str| {
        std::env::var(name)
            .ok()
            .map(|v| v.trim().trim_end_matches('/').to_string())
            .filter(|v| !v.is_empty())
    };
    var("APP_PUBLIC_URL")
        .or_else(|| {
            var("OAUTH_REDIRECT_BASE")
                .map(|base| base.strip_suffix("/api").unwrap_or(&base).to_string())
        })
        .unwrap_or_else(|| "http://localhost:8080".into())
}

/// A missing or malformed OAuth configuration value.
#[derive(Debug)]
pub(crate) struct OAuthConfigError(String);

impl std::fmt::Display for OAuthConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "oauth configuration error: {}", self.0)
    }
}

impl std::error::Error for OAuthConfigError {}

/// Which payment provider is active — selected by environment, so switching
/// between the local `fake` and real `stripe` never recompiles any domain or
/// application logic (bead authapp-d47ce3).
#[derive(Debug, Clone)]
pub(crate) enum PaymentProviderConfig {
    /// No provider configured; the payment surface is inactive.
    Disabled,
    /// The deterministic in-memory provider (local dev / integration tests).
    Fake,
    /// The Stripe adapter, in whichever mode its secret key selects.
    Stripe(StripeConfig),
}

impl PaymentProviderConfig {
    /// Read the active provider from `PAYMENT_PROVIDER`:
    ///
    /// - unset / empty / `none` → [`Self::Disabled`]
    /// - `fake` → [`Self::Fake`]
    /// - `stripe` → [`Self::Stripe`], requiring `STRIPE_SECRET_KEY`
    ///   (`STRIPE_API_BASE` optional, defaults to `https://api.stripe.com`)
    ///
    /// An unrecognized value is an error, never a silent fallback.
    pub(crate) fn from_env() -> Result<Self, PaymentConfigError> {
        let raw = std::env::var("PAYMENT_PROVIDER").unwrap_or_default();
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "none" | "disabled" => Ok(Self::Disabled),
            "fake" => Ok(Self::Fake),
            "stripe" => {
                let secret_key = std::env::var("STRIPE_SECRET_KEY")
                    .ok()
                    .filter(|v| !v.trim().is_empty())
                    .ok_or_else(|| {
                        PaymentConfigError(
                            "STRIPE_SECRET_KEY is required for PAYMENT_PROVIDER=stripe".into(),
                        )
                    })?;
                let mut config = StripeConfig::new(secret_key);
                if let Some(base) = std::env::var("STRIPE_API_BASE")
                    .ok()
                    .filter(|v| !v.trim().is_empty())
                {
                    config.api_base = base;
                }
                Ok(Self::Stripe(config))
            }
            other => Err(PaymentConfigError(format!(
                "unknown PAYMENT_PROVIDER {other:?} (expected fake|stripe|none)"
            ))),
        }
    }

    /// A short label for logs.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Fake => "fake",
            Self::Stripe(_) => "stripe",
        }
    }
}

/// A missing or malformed payment configuration value.
#[derive(Debug)]
pub(crate) struct PaymentConfigError(String);

impl std::fmt::Display for PaymentConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "payment configuration error: {}", self.0)
    }
}

impl std::error::Error for PaymentConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payment_provider_labels() {
        assert_eq!(PaymentProviderConfig::Disabled.label(), "disabled");
        assert_eq!(PaymentProviderConfig::Fake.label(), "fake");
        assert_eq!(
            PaymentProviderConfig::Stripe(StripeConfig::new("sk_test")).label(),
            "stripe"
        );
    }
}
