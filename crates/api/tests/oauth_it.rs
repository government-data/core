//! End-to-end test for OAuth/OIDC sign-in (bead authapp-541886): drives the
//! real router through the real, config-driven [`OidcProvider`] — its token
//! form, its id_token nonce/issuer/audience/expiry validation, its userinfo
//! parsing — against a real, ephemeral, freshly migrated Postgres (pending
//! store, identity linking, session issuance). Only the socket to the provider
//! is faked, via a scripted [`HttpClient`]; everything the provider adapter
//! actually does runs for real.
//!
//! Secrets are fixed (a test-only [`OAuthSecretGenerator`]) so the fake token
//! response can carry an id_token whose `nonce` matches the one `begin`
//! generated — the whole point of the nonce check.

use std::sync::Arc;

use application::HealthService;
use auth_gateway::LoginService;
use auth_gateway::{
    AdminRepository, NewAdmin, OAuthProvider, OAuthSecretGenerator, PasswordHash, PkcePair,
    ProviderId,
};
use auth_gateway::{AuditService, OAuthLoginService, SessionService};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use domain::{Email, Role};
use infrastructure::{
    oauth_http::{HttpClient, HttpError, HttpResponse},
    Argon2Hasher, Argon2Params, OidcConfig, OidcProvider, PgAdminRepository, PgAuditRepository,
    PgHealthCheck, PgIpLockoutStore, PgOAuthIdentityRepository, PgPendingAuthStore,
    PgSessionRepository, SecureRandomTokens, SystemClock,
};
use tower::ServiceExt;

const ADMIN_EMAIL: &str = "admin@example.com";
const FIXED_STATE: &str = "fixed-state-value";
const FIXED_NONCE: &str = "fixed-nonce-value";
const CLIENT_ID: &str = "client-123";
const ISSUER: &str = "https://accounts.example.com";

/// Fixed secrets so the scripted id_token's nonce can match.
struct FixedSecrets;
impl OAuthSecretGenerator for FixedSecrets {
    fn state(&self) -> String {
        FIXED_STATE.to_string()
    }
    fn nonce(&self) -> String {
        FIXED_NONCE.to_string()
    }
    fn pkce(&self) -> PkcePair {
        PkcePair::new("fixed-verifier", "fixed-challenge")
    }
    fn challenge(&self, verifier: &str) -> String {
        format!("challenge-of-{verifier}")
    }
}

/// A scripted transport: the token endpoint returns an id_token carrying
/// `FIXED_NONCE`, the userinfo endpoint returns `email` (by default the
/// admin's, verified).
struct ScriptedHttp {
    email: &'static str,
    verified: bool,
}

impl Default for ScriptedHttp {
    fn default() -> Self {
        Self {
            email: ADMIN_EMAIL,
            verified: true,
        }
    }
}

fn fake_id_token() -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256"}"#);
    let payload = serde_json::json!({
        "nonce": FIXED_NONCE,
        "iss": ISSUER,
        "aud": CLIENT_ID,
        "exp": 9_999_999_999i64,
    });
    let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
    format!("{header}.{payload}.signature")
}

#[async_trait::async_trait]
impl HttpClient for ScriptedHttp {
    async fn post_form(
        &self,
        _url: &str,
        _form: &[(String, String)],
        _bearer: Option<&str>,
    ) -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse {
            status: 200,
            body: serde_json::json!({
                "access_token": "fake-access-token",
                "id_token": fake_id_token(),
            })
            .to_string(),
        })
    }
    async fn get_bearer(&self, _url: &str, _bearer: &str) -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse {
            status: 200,
            body: serde_json::json!({
                "sub": "google-subject-1",
                "email": self.email,
                "email_verified": self.verified,
                "picture": "https://lh3.cdn.example.com/foto=s96-c",
            })
            .to_string(),
        })
    }
    async fn get_bytes(
        &self,
        _url: &str,
        _bearer: Option<&str>,
        _max: usize,
    ) -> Result<(String, Vec<u8>), HttpError> {
        Ok(("image/jpeg".into(), PICTURE.to_vec()))
    }
}

/// The picture the provider gives.
const PICTURE: &[u8] = &[0xff, 0xd8, 0xff, 0xe0];

fn oidc_config() -> OidcConfig {
    OidcConfig {
        provider_id: ProviderId::parse("google").unwrap(),
        client_id: CLIENT_ID.to_string(),
        client_secret: "secret".to_string(),
        auth_endpoint: "https://accounts.example.com/authorize".to_string(),
        token_endpoint: "https://accounts.example.com/token".to_string(),
        userinfo_endpoint: "https://accounts.example.com/userinfo".to_string(),
        issuer: ISSUER.to_string(),
        scopes: vec!["openid".to_string(), "email".to_string()],
        trust_email: false,
    }
}

async fn router(pool: sqlx::PgPool) -> axum::Router {
    router_with(pool, ScriptedHttp::default()).await
}

async fn router_with(pool: sqlx::PgPool, http: ScriptedHttp) -> axum::Router {
    let health = HealthService::new(Arc::new(PgHealthCheck::new(pool.clone())));
    let login = LoginService::new(
        Arc::new(PgAdminRepository::new(pool.clone())),
        Arc::new(PgIpLockoutStore::new(pool.clone())),
        Arc::new(Argon2Hasher::new(Argon2Params::owasp_default()).unwrap()),
        Arc::new(SystemClock),
        auth_gateway::LockoutPolicy::recommended(),
    );
    let sessions = SessionService::new(
        Arc::new(PgSessionRepository::new(pool.clone())),
        Arc::new(SecureRandomTokens),
        Arc::new(SystemClock),
        auth_gateway::SessionPolicy::recommended(),
    );
    let audit = AuditService::new(Arc::new(PgAuditRepository::new(pool.clone())));

    let provider: Arc<dyn OAuthProvider> =
        Arc::new(OidcProvider::new(oidc_config(), Arc::new(http)));
    let oauth = OAuthLoginService::new(
        vec![provider],
        Arc::new(PgPendingAuthStore::new(pool.clone())),
        Arc::new(PgOAuthIdentityRepository::new(pool.clone())),
        Arc::new(PgAdminRepository::new(pool.clone())),
        Arc::new(FixedSecrets),
        Arc::new(SystemClock),
        "https://app.example.com",
    );

    api::router(
        health,
        login,
        sessions,
        audit,
        Arc::new(PgAdminRepository::new(pool.clone())),
        Some((oauth, api::oauth::OAuthRedirects::default())),
        None,
        None,
        application::PlatformService::new(Arc::new(infrastructure::PgPlatformRepository::new(
            pool.clone(),
        ))),
        None,
        None,
        &[],
        api::rate_limit::RateLimitConfig {
            max_requests: 100,
            window: std::time::Duration::from_secs(60),
        },
    )
}

async fn seed_admin(pool: &sqlx::PgPool) -> domain::AdminId {
    // The account exists so OAuth can link/authenticate against it; its
    // password is never used on the OAuth path.
    let password_hash = PasswordHash::from_encoded("argon2-placeholder");
    PgAdminRepository::new(pool.clone())
        .insert(&NewAdmin {
            email: Email::parse(ADMIN_EMAIL).unwrap(),
            password_hash,
            role: Role::admin(),
        })
        .await
        .unwrap()
}

/// A signed-in admin's `Cookie` header value (session + csrf) and the CSRF
/// token to echo on mutations.
async fn signed_in(pool: &sqlx::PgPool, admin_id: domain::AdminId) -> (String, String) {
    let sessions = SessionService::new(
        Arc::new(PgSessionRepository::new(pool.clone())),
        Arc::new(SecureRandomTokens),
        Arc::new(SystemClock),
        auth_gateway::SessionPolicy::recommended(),
    );
    let issued = sessions.start(admin_id, Role::admin()).await.unwrap();
    let csrf = issued.csrf_token.as_str().to_string();
    (
        format!("session={}; csrf={csrf}", issued.token.as_str()),
        csrf,
    )
}

/// The value a response sets for cookie `name`, if it sets it.
fn set_cookie(response: &axum::http::Response<Body>, name: &str) -> Option<String> {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|s| s.starts_with(&format!("{name}=")))
        .map(str::to_string)
}

fn get(uri: &str, cookie: Option<&str>) -> Request<Body> {
    let mut req = Request::builder().uri(uri);
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    req.body(Body::empty()).unwrap()
}

async fn json(response: axum::http::Response<Body>) -> serde_json::Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn location(response: &axum::http::Response<Body>) -> String {
    response
        .headers()
        .get("location")
        .expect("a redirect must carry a Location header")
        .to_str()
        .unwrap()
        .to_string()
}

fn has_session_cookie(response: &axum::http::Response<Body>) -> bool {
    response.headers().get_all("set-cookie").iter().any(|v| {
        v.to_str()
            .map(|s| s.starts_with("session="))
            .unwrap_or(false)
    })
}

#[tokio::test]
async fn start_redirects_to_the_provider_with_pkce_and_state() {
    let db = testkit::spawn_test_db().await;
    let app = router(db.pool.clone()).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/oauth/google/start")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(response.status().is_redirection());
    let url = location(&response);
    assert!(url.starts_with("https://accounts.example.com/authorize?"));
    assert!(url.contains(&format!("state={FIXED_STATE}")));
    assert!(url.contains("code_challenge=fixed-challenge"));
    assert!(url.contains("code_challenge_method=S256"));
    // The redirect_uri is derived from the configured base + provider.
    assert!(url
        .contains("redirect_uri=https%3A%2F%2Fapp.example.com%2Fauth%2Foauth%2Fgoogle%2Fcallback"));
}

#[tokio::test]
async fn providers_lists_the_configured_provider_ids() {
    let db = testkit::spawn_test_db().await;
    let app = router(db.pool.clone()).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/oauth/providers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["providers"], serde_json::json!(["google"]));
}

#[tokio::test]
async fn start_for_an_unknown_provider_is_404() {
    let db = testkit::spawn_test_db().await;
    let app = router(db.pool.clone()).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/oauth/facebook/start")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // TODO(0034): split
async fn full_flow_links_the_identity_and_issues_a_session() {
    let db = testkit::spawn_test_db().await;
    let admin_id = seed_admin(&db.pool).await;
    let app = router(db.pool.clone()).await;

    // 1. Start: persists the pending authorization (state -> nonce/verifier).
    let start = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/oauth/google/start")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(start.status().is_redirection());

    // 2. Callback: the real OidcProvider exchanges the code (validating the
    //    id_token nonce/iss/aud/exp) and userinfo, links the identity, and a
    //    session is issued.
    let callback = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/auth/oauth/google/callback?state={FIXED_STATE}&code=the-code"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(callback.status().is_redirection());
    assert_eq!(location(&callback), "/");
    assert!(
        has_session_cookie(&callback),
        "a successful callback must set the session cookie"
    );

    // The external identity is now linked in its own table.
    let linked: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM admin_oauth_identities WHERE subject = $1")
            .bind("google-subject-1")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(linked, 1);

    // The provider's picture is now the profile picture.
    let (cookie, _) = signed_in(&db.pool, admin_id.clone()).await;
    let me = json(
        app.clone()
            .oneshot(get("/auth/me", Some(&cookie)))
            .await
            .unwrap(),
    )
    .await;
    assert!(me["avatar"].as_i64().is_some(), "{me}");
    let avatar = app
        .clone()
        .oneshot(get("/auth/me/avatar", Some(&cookie)))
        .await
        .unwrap();
    assert_eq!(avatar.status(), StatusCode::OK);
    assert_eq!(avatar.headers()["content-type"], "image/jpeg");
    assert_eq!(avatar.headers()["x-content-type-options"], "nosniff");
    let bytes = axum::body::to_bytes(avatar.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), PICTURE);

    // Their own picture (Mi cuenta): only an image, small, and it wins over
    // the provider's when signing in again.
    let (cookie, csrf) = signed_in(&db.pool, admin_id.clone()).await;
    let put = |body: Vec<u8>| {
        Request::builder()
            .method("PUT")
            .uri("/auth/me/avatar")
            .header("cookie", &cookie)
            .header("x-csrf-token", &csrf)
            .body(Body::from(body))
            .unwrap()
    };
    let png = [
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A][..],
        &[0; 8],
    ]
    .concat();
    for (body, status) in [
        (b"<svg onload=alert(1)>".to_vec(), StatusCode::BAD_REQUEST),
        (vec![0xff; 300 * 1024], StatusCode::BAD_REQUEST),
        (png.clone(), StatusCode::NO_CONTENT),
    ] {
        let r = app.clone().oneshot(put(body)).await.unwrap();
        assert_eq!(r.status(), status);
    }
    let own: (Option<String>, bool) =
        sqlx::query_as("SELECT avatar_type, avatar_own FROM admin_users WHERE id = $1::uuid")
            .bind(admin_id.as_str())
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(own, (Some("image/png".into()), true));
    // The provider's, as at the next sign-in, does not replace it.
    infrastructure::PgAdminRepository::new(db.pool.clone())
        .set_avatar(
            &admin_id,
            Some(&auth_gateway::ProfilePicture {
                content_type: "image/jpeg".into(),
                bytes: PICTURE.to_vec(),
            }),
        )
        .await
        .unwrap();
    let kept: Vec<u8> = sqlx::query_scalar("SELECT avatar FROM admin_users WHERE id = $1::uuid")
        .bind(admin_id.as_str())
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(kept, png);
    let removed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/auth/me/avatar")
                .header("cookie", &cookie)
                .header("x-csrf-token", &csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);
    let gone: (Option<Vec<u8>>, bool) =
        sqlx::query_as("SELECT avatar, avatar_own FROM admin_users WHERE id = $1::uuid")
            .bind(admin_id.as_str())
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(gone, (None, false));

    // Without a session, nothing.
    assert_eq!(
        app.oneshot(get("/auth/me/avatar", None))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn callback_with_an_unknown_state_redirects_to_failure_without_a_session() {
    let db = testkit::spawn_test_db().await;
    seed_admin(&db.pool).await;
    let app = router(db.pool.clone()).await;

    // No `start` first, so this state was never issued.
    let callback = app
        .oneshot(
            Request::builder()
                .uri("/auth/oauth/google/callback?state=never-issued&code=the-code")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(callback.status().is_redirection());
    assert!(location(&callback).starts_with("/login"));
    assert!(location(&callback).contains("error=oauth"));
    assert!(!has_session_cookie(&callback));
}

#[tokio::test]
async fn callback_with_a_provider_error_redirects_to_failure() {
    let db = testkit::spawn_test_db().await;
    let app = router(db.pool.clone()).await;

    let callback = app
        .oneshot(
            Request::builder()
                .uri("/auth/oauth/google/callback?error=access_denied")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(callback.status().is_redirection());
    assert!(location(&callback).contains("error=oauth"));
}

#[tokio::test]
async fn an_unverified_email_does_not_sign_in_by_email() {
    let db = testkit::spawn_test_db().await;
    seed_admin(&db.pool).await;
    let app = router_with(
        db.pool.clone(),
        ScriptedHttp {
            email: ADMIN_EMAIL,
            verified: false,
        },
    )
    .await;

    app.clone()
        .oneshot(get("/auth/oauth/google/start", None))
        .await
        .unwrap();
    let callback = app
        .oneshot(get(
            &format!("/auth/oauth/google/callback?state={FIXED_STATE}&code=c"),
            None,
        ))
        .await
        .unwrap();
    assert!(location(&callback).contains("error=oauth"));
    assert!(!has_session_cookie(&callback));
}

#[tokio::test]
async fn a_signed_in_admin_links_an_account_with_another_email_then_unlinks_it() {
    let db = testkit::spawn_test_db().await;
    let admin_id = seed_admin(&db.pool).await;
    let app = router_with(
        db.pool.clone(),
        ScriptedHttp {
            email: "someone.personal@gmail.com",
            verified: false,
        },
    )
    .await;
    let (cookies, csrf) = signed_in(&db.pool, admin_id).await;

    // Linking needs a session.
    let anonymous = app
        .clone()
        .oneshot(get("/auth/oauth/google/link", None))
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    // 1. Start the link: off to the provider, with the browser pinned.
    let start = app
        .clone()
        .oneshot(get("/auth/oauth/google/link", Some(&cookies)))
        .await
        .unwrap();
    assert!(start.status().is_redirection());
    assert!(location(&start).contains(&format!("state={FIXED_STATE}")));
    let pin = set_cookie(&start, "oauth_link").expect("the link cookie");
    assert!(pin.contains("SameSite=Lax") && pin.contains("HttpOnly"));

    // 2. The provider sends the browser back: only the Lax link cookie comes
    //    along (the session cookie is Strict).
    let callback = app
        .clone()
        .oneshot(get(
            &format!("/auth/oauth/google/callback?state={FIXED_STATE}&code=c"),
            Some(&format!("oauth_link={FIXED_STATE}")),
        ))
        .await
        .unwrap();
    assert_eq!(location(&callback), "/settings/account?linked=google");
    assert!(!has_session_cookie(&callback), "linking is not a sign-in");

    // 3. It shows on the account page.
    let list = app
        .clone()
        .oneshot(get("/auth/me/identities", Some(&cookies)))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list = json(list).await;
    assert_eq!(list["identities"][0]["provider"], "google");
    assert_eq!(list["identities"][0]["subject"], "google-subject-1");
    assert_eq!(list["identities"][0]["email"], "someone.personal@gmail.com");

    // 4. And now that account signs in, despite its unverified, foreign email.
    app.clone()
        .oneshot(get("/auth/oauth/google/start", None))
        .await
        .unwrap();
    let sign_in = app
        .clone()
        .oneshot(get(
            &format!("/auth/oauth/google/callback?state={FIXED_STATE}&code=c"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(location(&sign_in), "/");
    assert!(has_session_cookie(&sign_in));

    // 5. Unlink (a mutation: CSRF required).
    let uri = "/auth/me/identities/google/google-subject-1";
    let no_csrf = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(uri)
                .header("cookie", &cookies)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(no_csrf.status(), StatusCode::FORBIDDEN);
    let unlink = || {
        Request::builder()
            .method("DELETE")
            .uri(uri)
            .header("cookie", &cookies)
            .header("x-csrf-token", &csrf)
            .body(Body::empty())
            .unwrap()
    };
    let gone = app.clone().oneshot(unlink()).await.unwrap();
    assert_eq!(gone.status(), StatusCode::NO_CONTENT);
    let again = app.clone().oneshot(unlink()).await.unwrap();
    assert_eq!(again.status(), StatusCode::NOT_FOUND);

    let events: Vec<String> = sqlx::query_scalar(
        "SELECT event_type FROM admin_audit_events ORDER BY occurred_at, event_type",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert!(events.contains(&"oauth_linked".to_string()));
    assert!(events.contains(&"oauth_unlinked".to_string()));
}

#[tokio::test]
async fn a_link_callback_without_the_browser_pin_links_nothing() {
    let db = testkit::spawn_test_db().await;
    let admin_id = seed_admin(&db.pool).await;
    let app = router(db.pool.clone()).await;
    let (cookies, _) = signed_in(&db.pool, admin_id).await;

    app.clone()
        .oneshot(get("/auth/oauth/google/link", Some(&cookies)))
        .await
        .unwrap();
    // Someone else's browser (no link cookie) lands on the callback.
    let callback = app
        .oneshot(get(
            &format!("/auth/oauth/google/callback?state={FIXED_STATE}&code=c"),
            None,
        ))
        .await
        .unwrap();
    assert!(location(&callback).contains("error=oauth"));
    assert!(!has_session_cookie(&callback));
    let linked: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM admin_oauth_identities")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(linked, 0);
}
