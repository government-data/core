//! A generic, config-driven OIDC provider adapter implementing the
//! [`OAuthProvider`] port.
//!
//! One adapter serves any standard OIDC provider (Google, an in-house
//! Keycloak, ...): pointing it at a new provider is *configuration*
//! ([`OidcConfig`] — endpoints, client id/secret, scopes), never new code,
//! which is exactly the "add a provider = config, without touching the core"
//! acceptance criterion. A provider that is not OIDC (GitHub, say) is a new
//! `impl OAuthProvider` next to this one — the trait, not this struct, is the
//! extension point.
//!
//! The token/userinfo HTTP calls go through the [`HttpClient`] seam, so this
//! adapter's request-building and response-parsing are unit-tested against a
//! fake transport (no network); production injects [`ReqwestHttpClient`].

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use auth_gateway::{
    AuthorizeParams, ExchangeRequest, OAuthError, OAuthIdentity, OAuthProvider, ProfilePicture,
    ProviderId,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::Deserialize;

use crate::oauth_http::HttpClient;

/// Configuration for one OIDC provider. Everything here is per-provider and
/// comes from the environment — swapping providers never recompiles.
#[derive(Debug, Clone)]
pub struct OidcConfig {
    /// The provider id (`"google"`), matching the callback path segment.
    pub provider_id: ProviderId,
    /// The OAuth client id.
    pub client_id: String,
    /// The OAuth client secret.
    pub client_secret: String,
    /// The authorization endpoint (where the browser is sent).
    pub auth_endpoint: String,
    /// The token endpoint (server-to-server code exchange).
    pub token_endpoint: String,
    /// The userinfo endpoint (server-to-server, bearer access token).
    pub userinfo_endpoint: String,
    /// The `iss` the id_token must carry (issuer validation).
    pub issuer: String,
    /// The scopes to request (`openid`, `email`, ...).
    pub scopes: Vec<String>,
    /// Treat the reported email as verified even without an
    /// `email_verified: true` claim. For providers that never send the claim
    /// but only hand out addresses they own (a single-tenant Entra ID, say);
    /// left off, such an identity can sign in only once linked by hand.
    pub trust_email: bool,
}

/// A generic OIDC [`OAuthProvider`].
pub struct OidcProvider {
    config: OidcConfig,
    http: Arc<dyn HttpClient>,
}

impl std::fmt::Debug for OidcProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcProvider")
            .field("provider_id", &self.config.provider_id)
            .field("issuer", &self.config.issuer)
            .finish_non_exhaustive()
    }
}

impl OidcProvider {
    /// Build the provider from its config and an HTTP transport.
    pub fn new(config: OidcConfig, http: Arc<dyn HttpClient>) -> Self {
        Self { config, http }
    }
}

/// The subset of the token response we consume.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    id_token: String,
}

/// The id_token claims we validate.
#[derive(Debug, Deserialize)]
struct IdTokenClaims {
    iss: String,
    #[serde(default)]
    aud: Audience,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    exp: Option<i64>,
}

/// `aud` may be a single string or an array of strings.
#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
    #[default]
    None,
}

impl Audience {
    fn contains(&self, value: &str) -> bool {
        match self {
            Self::One(s) => s == value,
            Self::Many(v) => v.iter().any(|s| s == value),
            Self::None => false,
        }
    }
}

/// The subset of the userinfo response we consume.
#[derive(Debug, Deserialize)]
struct UserInfo {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<Verified>,
    /// The profile picture's URL (OIDC `picture`).
    #[serde(default)]
    picture: Option<String>,
}

/// `email_verified` is a boolean in the spec, but some providers (AWS
/// Cognito, older Keycloak mappers) send the string `"true"`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Verified {
    Bool(bool),
    Text(String),
}

impl Verified {
    fn is_true(&self) -> bool {
        match self {
            Self::Bool(b) => *b,
            Self::Text(s) => s.eq_ignore_ascii_case("true"),
        }
    }
}

#[async_trait]
impl OAuthProvider for OidcProvider {
    fn id(&self) -> ProviderId {
        self.config.provider_id.clone()
    }

    fn authorize_url(&self, params: &AuthorizeParams) -> String {
        // Build with `reqwest::Url` so every value is correctly percent-encoded.
        let scope = self.config.scopes.join(" ");
        let query = [
            ("response_type", "code"),
            ("client_id", self.config.client_id.as_str()),
            ("redirect_uri", params.redirect_uri.as_str()),
            ("scope", scope.as_str()),
            ("state", params.state.as_str()),
            ("nonce", params.nonce.as_str()),
            ("code_challenge", params.code_challenge.as_str()),
            ("code_challenge_method", "S256"),
        ];
        match reqwest::Url::parse_with_params(&self.config.auth_endpoint, query) {
            Ok(url) => url.to_string(),
            // A malformed configured endpoint is a config error; surfacing it
            // as an obviously-broken URL is fine here since `begin` validated
            // the provider exists, and the browser redirect will simply fail
            // loudly rather than silently.
            Err(_) => self.config.auth_endpoint.clone(),
        }
    }

    async fn exchange_code(&self, request: &ExchangeRequest) -> Result<OAuthIdentity, OAuthError> {
        // 1. Redeem the code (with the PKCE verifier) at the token endpoint.
        let form = vec![
            ("grant_type".to_string(), "authorization_code".to_string()),
            ("code".to_string(), request.code.clone()),
            ("redirect_uri".to_string(), request.redirect_uri.clone()),
            ("client_id".to_string(), self.config.client_id.clone()),
            (
                "client_secret".to_string(),
                self.config.client_secret.clone(),
            ),
            ("code_verifier".to_string(), request.code_verifier.clone()),
        ];
        let response = self
            .http
            .post_form(&self.config.token_endpoint, &form, None)
            .await
            .map_err(|e| OAuthError::Provider(e.to_string()))?;
        if !response.is_success() {
            return Err(OAuthError::ExchangeRejected(format!(
                "token endpoint returned {}",
                response.status
            )));
        }
        let token: TokenResponse = serde_json::from_str(&response.body)
            .map_err(|e| OAuthError::Provider(format!("token response: {e}")))?;

        // 2. Validate the id_token's claims (nonce, issuer, audience, expiry).
        //    NOTE: this decodes the id_token payload and validates its claims
        //    but does not yet verify its RS256 signature against the provider's
        //    JWKS. That is safe *here* because the id_token arrives directly
        //    from the token endpoint over TLS (not via the browser), and the
        //    identity of record below comes from the userinfo endpoint (also a
        //    direct, bearer-authenticated TLS call). JWKS signature
        //    verification is the documented remaining hardening step.
        self.validate_id_token(&token.id_token, &request.expected_nonce)?;

        // 3. The identity of record comes from userinfo (subject + email).
        let userinfo = self
            .http
            .get_bearer(&self.config.userinfo_endpoint, &token.access_token)
            .await
            .map_err(|e| OAuthError::Provider(e.to_string()))?;
        if !userinfo.is_success() {
            return Err(OAuthError::Provider(format!(
                "userinfo endpoint returned {}",
                userinfo.status
            )));
        }
        let info: UserInfo = serde_json::from_str(&userinfo.body)
            .map_err(|e| OAuthError::Provider(format!("userinfo response: {e}")))?;

        let email = info
            .email
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty());
        let email_verified = email.is_some()
            && match info.email_verified {
                Some(v) => v.is_true(),
                None => self.config.trust_email,
            };
        let picture = match info.picture.as_deref() {
            Some(url) => self.fetch_picture(url, &token.access_token).await,
            None => None,
        };
        Ok(OAuthIdentity {
            provider: self.config.provider_id.clone(),
            subject: info.sub,
            email,
            email_verified,
            picture,
        })
    }
}

impl OidcProvider {
    /// Fetch the profile picture, best effort: a picture that cannot be had
    /// (not https, not an accepted image, too big, unreachable) is skipped,
    /// never a reason to refuse the sign-in. The access token goes along only
    /// to the userinfo host (Entra ID serves the photo from Graph); a picture
    /// on another host (Google's CDN) is fetched without it.
    async fn fetch_picture(&self, url: &str, access_token: &str) -> Option<ProfilePicture> {
        let parsed = reqwest::Url::parse(url)
            .ok()
            .filter(|u| u.scheme() == "https")?;
        let same_host = reqwest::Url::parse(&self.config.userinfo_endpoint)
            .ok()
            .is_some_and(|u| u.host_str() == parsed.host_str());
        let bearer = same_host.then_some(access_token);
        match self
            .http
            .get_bytes(parsed.as_str(), bearer, ProfilePicture::MAX_BYTES)
            .await
        {
            Ok((content_type, bytes))
                if ProfilePicture::accepts(&content_type) && !bytes.is_empty() =>
            {
                Some(ProfilePicture {
                    content_type,
                    bytes,
                })
            }
            Ok((content_type, _)) => {
                tracing::debug!(%content_type, "profile picture skipped: not an accepted image");
                None
            }
            Err(e) => {
                tracing::debug!("profile picture skipped: {e}");
                None
            }
        }
    }
}

impl OidcProvider {
    /// Decode the id_token payload and validate `nonce`, `iss`, `aud`, `exp`.
    fn validate_id_token(&self, id_token: &str, expected_nonce: &str) -> Result<(), OAuthError> {
        let payload = id_token
            .split('.')
            .nth(1)
            .ok_or_else(|| OAuthError::ExchangeRejected("id_token is not a JWT".into()))?;
        let bytes = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|e| OAuthError::ExchangeRejected(format!("id_token payload: {e}")))?;
        let claims: IdTokenClaims = serde_json::from_slice(&bytes)
            .map_err(|e| OAuthError::ExchangeRejected(format!("id_token claims: {e}")))?;

        match claims.nonce.as_deref() {
            Some(n) if n == expected_nonce => {}
            _ => {
                return Err(OAuthError::ExchangeRejected(
                    "id_token nonce mismatch".into(),
                ))
            }
        }
        if claims.iss != self.config.issuer {
            return Err(OAuthError::ExchangeRejected(
                "id_token issuer mismatch".into(),
            ));
        }
        if !claims.aud.contains(&self.config.client_id) {
            return Err(OAuthError::ExchangeRejected(
                "id_token audience mismatch".into(),
            ));
        }
        if let Some(exp) = claims.exp {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            if exp <= now {
                return Err(OAuthError::ExchangeRejected("id_token has expired".into()));
            }
        }
        Ok(())
    }
}

/// The endpoints an OIDC issuer publishes in its discovery document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcEndpoints {
    pub issuer: String,
    pub auth_endpoint: String,
    pub token_endpoint: String,
    pub userinfo_endpoint: String,
}

#[derive(Debug, Deserialize)]
struct DiscoveryDocument {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: Option<String>,
}

/// Read `{issuer}/.well-known/openid-configuration`, so an admin types only
/// the issuer (for Entra ID, `https://login.microsoftonline.com/{tenant}/v2.0`).
pub async fn discover_oidc(issuer: &str) -> Result<OidcEndpoints, String> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim().trim_end_matches('/')
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("{url}: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("{url} respondió {status}"));
    }
    let body = response.text().await.map_err(|e| e.to_string())?;
    parse_discovery(&body)
}

fn parse_discovery(body: &str) -> Result<OidcEndpoints, String> {
    let doc: DiscoveryDocument =
        serde_json::from_str(body).map_err(|e| format!("no es un documento OIDC: {e}"))?;
    Ok(OidcEndpoints {
        issuer: doc.issuer,
        auth_endpoint: doc.authorization_endpoint,
        token_endpoint: doc.token_endpoint,
        userinfo_endpoint: doc
            .userinfo_endpoint
            .ok_or("el emisor no publica userinfo_endpoint")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::oauth_http::{HttpError, HttpResponse};

    fn config() -> OidcConfig {
        OidcConfig {
            provider_id: ProviderId::parse("google").unwrap(),
            client_id: "client-123".to_string(),
            client_secret: "secret-xyz".to_string(),
            auth_endpoint: "https://accounts.example.com/authorize".to_string(),
            token_endpoint: "https://accounts.example.com/token".to_string(),
            userinfo_endpoint: "https://accounts.example.com/userinfo".to_string(),
            issuer: "https://accounts.example.com".to_string(),
            scopes: vec!["openid".to_string(), "email".to_string()],
            trust_email: false,
        }
    }

    /// Build a fake id_token: a JWT with an unsigned header and the given
    /// payload claims (only the payload is read).
    fn fake_id_token(nonce: &str, iss: &str, aud: &str, exp: i64) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = serde_json::json!({
            "nonce": nonce, "iss": iss, "aud": aud, "exp": exp
        });
        let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
        format!("{header}.{payload}.sig")
    }

    /// A scripted HTTP client: returns the queued token response for the POST
    /// and the queued userinfo response for the GET.
    struct ScriptedHttp {
        token: HttpResponse,
        userinfo: HttpResponse,
        posted_form: Mutex<Option<Vec<(String, String)>>>,
        /// What a picture GET answers, and the (url, bearer) it was asked with.
        picture: Mutex<Option<(String, Vec<u8>)>>,
        picture_asked: Mutex<Vec<(String, Option<String>)>>,
    }

    #[async_trait]
    impl HttpClient for ScriptedHttp {
        async fn post_form(
            &self,
            _url: &str,
            form: &[(String, String)],
            _bearer: Option<&str>,
        ) -> Result<HttpResponse, HttpError> {
            *self.posted_form.lock().unwrap() = Some(form.to_vec());
            Ok(self.token.clone())
        }
        async fn get_bearer(&self, _url: &str, _bearer: &str) -> Result<HttpResponse, HttpError> {
            Ok(self.userinfo.clone())
        }
        async fn get_bytes(
            &self,
            url: &str,
            bearer: Option<&str>,
            _max: usize,
        ) -> Result<(String, Vec<u8>), HttpError> {
            self.picture_asked
                .lock()
                .unwrap()
                .push((url.to_string(), bearer.map(str::to_string)));
            self.picture
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| HttpError("no picture".into()))
        }
    }

    fn provider(token_body: String, userinfo_body: String) -> (OidcProvider, Arc<ScriptedHttp>) {
        provider_with(config(), token_body, userinfo_body)
    }

    fn provider_with(
        config: OidcConfig,
        token_body: String,
        userinfo_body: String,
    ) -> (OidcProvider, Arc<ScriptedHttp>) {
        let http = Arc::new(ScriptedHttp {
            token: HttpResponse {
                status: 200,
                body: token_body,
            },
            userinfo: HttpResponse {
                status: 200,
                body: userinfo_body,
            },
            posted_form: Mutex::new(None),
            picture: Mutex::new(None),
            picture_asked: Mutex::new(Vec::new()),
        });
        (OidcProvider::new(config, http.clone()), http)
    }

    #[test]
    fn authorize_url_carries_every_pkce_and_oidc_param() {
        let (provider, _) = provider(String::new(), String::new());
        let url = provider.authorize_url(&AuthorizeParams {
            state: "st".to_string(),
            nonce: "no".to_string(),
            code_challenge: "ch".to_string(),
            redirect_uri: "https://app.example.com/cb".to_string(),
        });
        assert!(url.starts_with("https://accounts.example.com/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=client-123"));
        assert!(url.contains("code_challenge=ch"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=st"));
        assert!(url.contains("nonce=no"));
        // The redirect URI is percent-encoded.
        assert!(url.contains("redirect_uri=https%3A%2F%2Fapp.example.com%2Fcb"));
    }

    fn exchange_request(nonce: &str) -> ExchangeRequest {
        ExchangeRequest {
            code: "the-code".to_string(),
            code_verifier: "the-verifier".to_string(),
            redirect_uri: "https://app.example.com/cb".to_string(),
            expected_nonce: nonce.to_string(),
        }
    }

    #[tokio::test]
    async fn exchange_returns_the_userinfo_identity_and_sends_pkce_verifier() {
        let id_token = fake_id_token(
            "expected-nonce",
            "https://accounts.example.com",
            "client-123",
            9_999_999_999,
        );
        let token_body =
            serde_json::json!({"access_token": "at", "id_token": id_token}).to_string();
        let userinfo_body =
            serde_json::json!({"sub": "google-sub-1", "email": "admin@example.com"}).to_string();
        let (provider, http) = provider(token_body, userinfo_body);

        let identity = provider
            .exchange_code(&exchange_request("expected-nonce"))
            .await
            .unwrap();
        assert_eq!(identity.subject, "google-sub-1");
        assert_eq!(identity.email.as_deref(), Some("admin@example.com"));
        // No `email_verified` claim, and the provider is not trusted.
        assert!(!identity.email_verified);
        assert_eq!(identity.provider, ProviderId::parse("google").unwrap());

        // The token POST carried the PKCE verifier (the whole point of PKCE).
        let form = http.posted_form.lock().unwrap().clone().unwrap();
        assert!(form
            .iter()
            .any(|(k, v)| k == "code_verifier" && v == "the-verifier"));
        assert!(form
            .iter()
            .any(|(k, v)| k == "grant_type" && v == "authorization_code"));
    }

    /// The identity for a userinfo body, with or without `trust_email`.
    async fn identity_for(userinfo: serde_json::Value, trust_email: bool) -> OAuthIdentity {
        let id_token = fake_id_token(
            "expected-nonce",
            "https://accounts.example.com",
            "client-123",
            9_999_999_999,
        );
        let token_body =
            serde_json::json!({"access_token": "at", "id_token": id_token}).to_string();
        let (provider, _) = provider_with(
            OidcConfig {
                trust_email,
                ..config()
            },
            token_body,
            userinfo.to_string(),
        );
        provider
            .exchange_code(&exchange_request("expected-nonce"))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn the_email_is_verified_only_when_the_provider_says_so_or_is_trusted() {
        let verified = |v: serde_json::Value| serde_json::json!({"sub": "s", "email": "a@example.com", "email_verified": v});
        assert!(
            identity_for(verified(true.into()), false)
                .await
                .email_verified
        );
        assert!(
            identity_for(verified("true".into()), false)
                .await
                .email_verified
        );
        assert!(
            !identity_for(verified(false.into()), false)
                .await
                .email_verified
        );
        // Trusting the provider does not override an explicit `false`.
        assert!(
            !identity_for(verified(false.into()), true)
                .await
                .email_verified
        );

        let unclaimed = serde_json::json!({"sub": "s", "email": "a@example.com"});
        assert!(!identity_for(unclaimed.clone(), false).await.email_verified);
        assert!(identity_for(unclaimed, true).await.email_verified);
    }

    #[tokio::test]
    async fn a_provider_without_email_still_yields_an_identity() {
        let identity = identity_for(serde_json::json!({"sub": "s"}), true).await;
        assert_eq!(identity.subject, "s");
        assert_eq!(identity.email, None);
        assert!(!identity.email_verified);
    }

    #[tokio::test]
    async fn exchange_rejects_a_nonce_mismatch() {
        let id_token = fake_id_token(
            "a-different-nonce",
            "https://accounts.example.com",
            "client-123",
            9_999_999_999,
        );
        let token_body =
            serde_json::json!({"access_token": "at", "id_token": id_token}).to_string();
        let (provider, _) = provider(token_body, String::new());

        assert!(matches!(
            provider
                .exchange_code(&exchange_request("expected-nonce"))
                .await,
            Err(OAuthError::ExchangeRejected(_))
        ));
    }

    #[tokio::test]
    async fn exchange_rejects_a_wrong_audience() {
        let id_token = fake_id_token(
            "expected-nonce",
            "https://accounts.example.com",
            "some-other-client",
            9_999_999_999,
        );
        let token_body =
            serde_json::json!({"access_token": "at", "id_token": id_token}).to_string();
        let (provider, _) = provider(token_body, String::new());

        assert!(matches!(
            provider
                .exchange_code(&exchange_request("expected-nonce"))
                .await,
            Err(OAuthError::ExchangeRejected(_))
        ));
    }

    #[tokio::test]
    async fn exchange_rejects_an_expired_id_token() {
        let id_token = fake_id_token(
            "expected-nonce",
            "https://accounts.example.com",
            "client-123",
            1, // 1970 — long expired
        );
        let token_body =
            serde_json::json!({"access_token": "at", "id_token": id_token}).to_string();
        let (provider, _) = provider(token_body, String::new());

        assert!(matches!(
            provider
                .exchange_code(&exchange_request("expected-nonce"))
                .await,
            Err(OAuthError::ExchangeRejected(_))
        ));
    }

    #[test]
    fn a_discovery_document_yields_the_endpoints() {
        let doc = r#"{"issuer":"https://login.microsoftonline.com/t/v2.0",
            "authorization_endpoint":"https://login.microsoftonline.com/t/oauth2/v2.0/authorize",
            "token_endpoint":"https://login.microsoftonline.com/t/oauth2/v2.0/token",
            "userinfo_endpoint":"https://graph.microsoft.com/oidc/userinfo"}"#;
        let e = parse_discovery(doc).unwrap();
        assert_eq!(e.issuer, "https://login.microsoftonline.com/t/v2.0");
        assert!(e.token_endpoint.ends_with("/token"));
        assert!(parse_discovery(
            r#"{"issuer":"x","authorization_endpoint":"a","token_endpoint":"t"}"#
        )
        .is_err());
    }

    #[tokio::test]
    async fn the_picture_is_kept_without_leaking_the_token_and_only_if_it_is_an_image() {
        let id_token = fake_id_token(
            "n",
            "https://accounts.example.com",
            "client-123",
            9_999_999_999,
        );
        let token_body =
            serde_json::json!({"access_token": "at", "id_token": id_token}).to_string();
        let on_cdn = serde_json::json!({
            "sub": "s", "picture": "https://lh3.cdn.example.com/a/foto=s96-c"
        })
        .to_string();

        // Google: on its CDN, fetched without the access token.
        let (oidc, http) = provider(token_body.clone(), on_cdn.clone());
        *http.picture.lock().unwrap() = Some(("image/jpeg".into(), vec![0xff, 0xd8, 0xff]));
        let identity = oidc.exchange_code(&exchange_request("n")).await.unwrap();
        let picture = identity.picture.unwrap();
        assert_eq!(picture.content_type, "image/jpeg");
        assert_eq!(picture.bytes, vec![0xff, 0xd8, 0xff]);
        assert_eq!(
            http.picture_asked.lock().unwrap().as_slice(),
            [("https://lh3.cdn.example.com/a/foto=s96-c".to_string(), None)]
        );

        // Entra ID style: on the userinfo host, with the token.
        let same_host = serde_json::json!({
            "sub": "s", "picture": "https://accounts.example.com/me/photo"
        })
        .to_string();
        let (oidc, http) = provider(token_body.clone(), same_host);
        *http.picture.lock().unwrap() = Some(("image/png".into(), vec![1]));
        let identity = oidc.exchange_code(&exchange_request("n")).await.unwrap();
        assert!(identity.picture.is_some());
        assert_eq!(
            http.picture_asked.lock().unwrap()[0].1.as_deref(),
            Some("at")
        );

        // An SVG, a failed fetch or a plain-http URL: no picture, still signed in.
        let (oidc, http) = provider(token_body.clone(), on_cdn);
        *http.picture.lock().unwrap() = Some(("image/svg+xml".into(), b"<svg/>".to_vec()));
        assert!(oidc
            .exchange_code(&exchange_request("n"))
            .await
            .unwrap()
            .picture
            .is_none());
        let plain =
            serde_json::json!({"sub": "s", "picture": "http://x.example.com/p"}).to_string();
        let (oidc, http) = provider(token_body, plain);
        assert!(oidc
            .exchange_code(&exchange_request("n"))
            .await
            .unwrap()
            .picture
            .is_none());
        assert!(http.picture_asked.lock().unwrap().is_empty());
    }
}
