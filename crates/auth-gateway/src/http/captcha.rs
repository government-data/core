//! Captcha (Cloudflare Turnstile) on what a bot could use to send emails or
//! guess passwords: «¿Olvidaste tu contraseña?» always, and signing in from an
//! IP's third failed attempt.
//!
//! It goes as an extension (`Extension<Captcha>`): without it, or without
//! keys, nothing is asked (customer installations and tests). The keys come
//! from the environment or from management (the online cell and the instances
//! we manage) and can be changed live. The server verifies with the secret
//! key: the browser only brings the token.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use serde::Serialize;

use http_kit::error::ApiError;

/// Whoever asks Turnstile whether a token is good.
#[async_trait]
pub trait TokenVerifier: Send + Sync {
    async fn verify(&self, secret: &str, token: &str, ip: &str) -> Result<bool, String>;
}

/// An IP's failed attempts before the captcha is required to sign in.
const FAILURES_BEFORE: u32 = 3;
/// How long those failures are remembered.
const WINDOW: Duration = Duration::from_secs(30 * 60);

#[derive(Clone)]
struct Keys {
    site: String,
    secret: String,
}

#[derive(Clone)]
pub struct Captcha {
    keys: Arc<RwLock<Option<Keys>>>,
    verifier: Arc<dyn TokenVerifier>,
    failures: Arc<Mutex<HashMap<String, (u32, Instant)>>>,
}

impl std::fmt::Debug for Captcha {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Captcha")
            .field("active", &self.site_key().is_some())
            .finish()
    }
}

impl Captcha {
    pub fn new(verifier: Arc<dyn TokenVerifier>) -> Self {
        Self {
            keys: Arc::default(),
            verifier,
            failures: Arc::default(),
        }
    }

    /// Set (or with `None`, remove) the keys: `(site, secret)`.
    pub fn configure(&self, keys: Option<(String, String)>) {
        *self.keys.write().unwrap_or_else(PoisonError::into_inner) =
            keys.map(|(site, secret)| Keys { site, secret });
    }

    /// The public key, for the browser; `None`: no captcha is asked.
    pub fn site_key(&self) -> Option<String> {
        self.keys
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|k| k.site.clone())
    }

    /// `Ok` if there is no captcha or the token is good.
    pub async fn verify(&self, token: Option<&str>, ip: &str) -> Result<(), ApiError> {
        let Some(keys) = self
            .keys
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        else {
            return Ok(());
        };
        let token = token.map(str::trim).filter(|t| !t.is_empty());
        let good = match token {
            Some(t) => match self.verifier.verify(&keys.secret, t, ip).await {
                Ok(good) => good,
                Err(e) => {
                    // If Turnstile does not answer it lets through: everyone is
                    // not blocked over a Cloudflare outage (the limits remain).
                    tracing::warn!("captcha: Turnstile did not answer: {e}");
                    true
                }
            },
            None => false,
        };
        if good {
            Ok(())
        } else {
            Err(required())
        }
    }

    fn failures_of(&self, ip: &str) -> u32 {
        let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        failures.retain(|_, (_, since)| since.elapsed() < WINDOW);
        failures.get(ip).map_or(0, |(n, _)| *n)
    }

    /// Whether signing in from `ip` needs the captcha.
    pub fn required_for_login(&self, ip: &str) -> bool {
        self.site_key().is_some() && self.failures_of(ip) >= FAILURES_BEFORE
    }

    pub fn record_failure(&self, ip: &str) {
        let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        let e = failures
            .entry(ip.to_string())
            .or_insert((0, Instant::now()));
        e.0 += 1;
        e.1 = Instant::now();
    }

    pub fn record_success(&self, ip: &str) {
        self.failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(ip);
    }
}

/// What the API says when the captcha is missing (or did not pass): the UI
/// shows it and sends again with the token.
pub fn required() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "captcha_required",
        "Confirm that you are not a robot.",
    )
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CaptchaOut {
    /// Turnstile's public key; `null`: no captcha is asked.
    site_key: Option<String>,
}

/// `GET /auth/captcha`: which key to render the captcha with, if any.
#[utoipa::path(
    get,
    path = "/auth/captcha",
    responses((status = 200, description = "The Turnstile site key, if a captcha is used", body = CaptchaOut)),
    tag = "auth",
)]
pub async fn captcha_handler(captcha: Option<Extension<Captcha>>) -> impl IntoResponse {
    Json(CaptchaOut {
        site_key: captcha.and_then(|Extension(c)| c.site_key()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(bool);

    #[async_trait]
    impl TokenVerifier for Fixed {
        async fn verify(&self, _: &str, token: &str, _: &str) -> Result<bool, String> {
            Ok(self.0 && token == "good")
        }
    }

    #[tokio::test]
    async fn without_keys_nothing_is_asked() {
        let c = Captcha::new(Arc::new(Fixed(false)));
        assert!(c.verify(None, "1.1.1.1").await.is_ok());
        for _ in 0..5 {
            c.record_failure("1.1.1.1");
        }
        assert!(!c.required_for_login("1.1.1.1"));
    }

    #[tokio::test]
    async fn with_keys_a_good_token_passes_and_failures_turn_it_on_for_login() {
        let c = Captcha::new(Arc::new(Fixed(true)));
        c.configure(Some(("site".into(), "secret".into())));
        assert_eq!(c.site_key().as_deref(), Some("site"));
        assert!(c.verify(None, "ip").await.is_err());
        assert!(c.verify(Some("bad"), "ip").await.is_err());
        assert!(c.verify(Some("good"), "ip").await.is_ok());
        assert!(!c.required_for_login("ip"));
        c.record_failure("ip");
        c.record_failure("ip");
        assert!(!c.required_for_login("ip"));
        c.record_failure("ip");
        assert!(c.required_for_login("ip"));
        assert!(!c.required_for_login("other"));
        c.record_success("ip");
        assert!(!c.required_for_login("ip"));
    }
}
