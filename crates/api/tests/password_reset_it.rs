//! End-to-end password recovery: drives the real `/auth/password-reset/*` and
//! `/auth/login` routes against a real, freshly migrated Postgres via
//! `testkit`. Only the outgoing mail is a fake, which keeps what it was asked
//! to send so the test can follow the link like a person would.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use application::{HealthService, MailSlot, Mailer, OutgoingMail};
use async_trait::async_trait;
use auth_gateway::PasswordPolicy;
use auth_gateway::{AuditService, SessionService};
use auth_gateway::{BootstrapService, LoginService, PasswordResetService};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain::AuditRepository;
use infrastructure::{
    Argon2Hasher, Argon2Params, PgAdminRepository, PgAuditRepository, PgHealthCheck,
    PgIpLockoutStore, PgPasswordResetStore, PgSessionRepository, ResetTokenAdapter,
    SecureRandomTokens, SystemClock,
};
use tower::ServiceExt;

const EMAIL: &str = "ana@example.com";
const OLD: &str = "Old-Passw0rd-123";
const NEW: &str = "Brand-New-Passw0rd";

#[derive(Default)]
struct Outbox(Mutex<Vec<OutgoingMail>>);

#[async_trait]
impl Mailer for Outbox {
    async fn send(&self, mail: &OutgoingMail) -> Result<(), String> {
        self.0.lock().unwrap().push(mail.clone());
        Ok(())
    }
}

impl Outbox {
    /// The mail sent in the background, once it has gone out.
    async fn wait(&self, n: usize) -> Vec<OutgoingMail> {
        for _ in 0..100 {
            if self.0.lock().unwrap().len() >= n {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.0.lock().unwrap().clone()
    }
}

fn hasher() -> Arc<Argon2Hasher> {
    Arc::new(Argon2Hasher::new(Argon2Params::owasp_default()).unwrap())
}

async fn app(pool: sqlx::PgPool, outbox: Arc<Outbox>) -> axum::Router {
    let admins = Arc::new(PgAdminRepository::new(pool.clone()));
    BootstrapService::new(admins.clone(), hasher(), PasswordPolicy::recommended())
        .create_first_admin(EMAIL, OLD)
        .await
        .unwrap();
    let login = LoginService::new(
        admins.clone(),
        Arc::new(PgIpLockoutStore::new(pool.clone())),
        hasher(),
        Arc::new(SystemClock),
        auth_gateway::LockoutPolicy::recommended(),
    );
    let sessions = SessionService::new(
        Arc::new(PgSessionRepository::new(pool.clone())),
        Arc::new(SecureRandomTokens),
        Arc::new(SystemClock),
        auth_gateway::SessionPolicy::recommended(),
    );
    let reset = PasswordResetService::new(
        admins.clone(),
        Arc::new(PgPasswordResetStore::new(pool.clone())),
        Arc::new(ResetTokenAdapter),
        hasher(),
        Arc::new(SystemClock),
        MailSlot::fixed(outbox),
        PasswordPolicy::recommended(),
        Duration::from_secs(30 * 60),
    );
    api::router(
        HealthService::new(Arc::new(PgHealthCheck::new(pool.clone()))),
        login,
        sessions,
        AuditService::new(Arc::new(PgAuditRepository::new(pool.clone()))),
        admins,
        None,
        None,
        None,
        application::PlatformService::new(Arc::new(infrastructure::PgPlatformRepository::new(
            pool.clone(),
        ))),
        None,
        Some((
            reset,
            api::password_reset::ResetLinkBase::Fixed("https://orq.example.com".into()),
        )),
        &[],
        api::rate_limit::RateLimitConfig {
            max_requests: 100,
            window: Duration::from_secs(60),
        },
    )
}

async fn post(app: &axum::Router, uri: &str, body: serde_json::Value) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .header("x-forwarded-for", "203.0.113.5")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn login(email: &str, password: &str) -> serde_json::Value {
    serde_json::json!({ "email": email, "password": password })
}

#[tokio::test]
async fn the_emailed_link_sets_a_new_password_once() {
    let db = testkit::spawn_test_db().await;
    let outbox = Arc::new(Outbox::default());
    let app = app(db.pool.clone(), outbox.clone()).await;

    let (status, body) = post(
        &app,
        "/auth/password-reset/request",
        serde_json::json!({ "email": "Ana@Example.com" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert!(body.contains("\"expires_in_minutes\":30"), "{body}");

    let mail = outbox.wait(1).await;
    assert_eq!(mail.len(), 1);
    assert_eq!(mail[0].to, vec![EMAIL.to_string()]);
    let prefix = "https://orq.example.com/reset-password#";
    let token = mail[0]
        .text
        .split_whitespace()
        .find_map(|w| w.strip_prefix(prefix))
        .expect("the mail carries the link")
        .to_string();
    assert_eq!(token.len(), 64);

    let (status, body) = post(
        &app,
        "/auth/password-reset/check",
        serde_json::json!({ "token": token }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"valid\":true"), "{body}");
    assert!(body.contains("\"min_length\":12"), "{body}");

    // Too weak: refused with the unmet rules, and the link keeps working.
    let (status, body) = post(
        &app,
        "/auth/password-reset/confirm",
        serde_json::json!({ "token": token, "password": "short" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("password"), "{body}");

    let (status, _) = post(
        &app,
        "/auth/password-reset/confirm",
        serde_json::json!({ "token": token, "password": NEW }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The old password no longer works; the new one does.
    let (status, _) = post(&app, "/auth/login", login(EMAIL, OLD)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = post(&app, "/auth/login", login(EMAIL, NEW)).await;
    assert_eq!(status, StatusCode::OK);

    // The link was single-use.
    let (status, _) = post(
        &app,
        "/auth/password-reset/confirm",
        serde_json::json!({ "token": token, "password": "Yet-Another-Passw0rd" }),
    )
    .await;
    assert_eq!(status, StatusCode::GONE);
    let (_, body) = post(
        &app,
        "/auth/password-reset/check",
        serde_json::json!({ "token": token }),
    )
    .await;
    assert!(body.contains("\"valid\":false"), "{body}");

    let events: Vec<String> = PgAuditRepository::new(db.pool.clone())
        .list_recent(50)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.event_type.as_str().to_string())
        .collect();
    assert!(events.contains(&"password_reset_requested".to_string()));
    assert!(events.contains(&"password_reset_completed".to_string()));
}

#[tokio::test]
async fn an_unknown_email_gets_the_same_answer_and_no_mail() {
    let db = testkit::spawn_test_db().await;
    let outbox = Arc::new(Outbox::default());
    let app = app(db.pool.clone(), outbox.clone()).await;

    let (status, body) = post(
        &app,
        "/auth/password-reset/request",
        serde_json::json!({ "email": "nobody@example.com" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(body.contains("\"expires_in_minutes\":30"), "{body}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(outbox.0.lock().unwrap().is_empty());

    let (status, _) = post(
        &app,
        "/auth/password-reset/request",
        serde_json::json!({ "email": "not-an-email" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

/// A fake Turnstile: only the «good» token passes.
struct Turnstile;

#[async_trait]
impl api::captcha::TokenVerifier for Turnstile {
    async fn verify(&self, secret: &str, token: &str, _ip: &str) -> Result<bool, String> {
        assert_eq!(secret, "secret");
        Ok(token == "good")
    }
}

#[tokio::test]
async fn with_a_captcha_bots_cannot_send_mail_or_keep_guessing() {
    let db = testkit::spawn_test_db().await;
    let outbox = Arc::new(Outbox::default());
    let captcha = api::captcha::Captcha::new(Arc::new(Turnstile));
    captcha.configure(Some(("site".into(), "secret".into())));
    let app = app(db.pool.clone(), outbox.clone())
        .await
        .layer(axum::Extension(captcha));

    // The UI knows which key to render.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/captcha")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&bytes), r#"{"site_key":"site"}"#);

    // «¿Olvidaste tu contraseña?»: without a token or with a bad one no mail goes out.
    let request = |captcha: Option<&str>| serde_json::json!({ "email": EMAIL, "captcha": captcha });
    let (status, body) = post(&app, "/auth/password-reset/request", request(None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.contains("captcha_required"), "{body}");
    let (status, _) = post(&app, "/auth/password-reset/request", request(Some("bad"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = post(&app, "/auth/password-reset/request", request(Some("good"))).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(outbox.wait(1).await.len(), 1);

    // Signing in: the first two failures, without captcha; from the third, it is required.
    for _ in 0..3 {
        let (status, _) = post(&app, "/auth/login", login(EMAIL, "mala-clave")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, body) = post(&app, "/auth/login", login(EMAIL, OLD)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("captcha_required"));
    let mut with_token = login(EMAIL, OLD);
    with_token["captcha"] = serde_json::json!("good");
    let (status, _) = post(&app, "/auth/login", with_token).await;
    assert_eq!(status, StatusCode::OK);
    // Signing in forgets the failures.
    let (status, _) = post(&app, "/auth/login", login(EMAIL, OLD)).await;
    assert_eq!(status, StatusCode::OK);
}
