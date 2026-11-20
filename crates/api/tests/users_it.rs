//! End-to-end Settings → Users: an admin creates an account, its
//! owner gets the activation link by email, cannot sign in until they open it,
//! chooses a password with it and signs in; then resending, permissions and
//! deleting. Real routes against a real, freshly migrated Postgres; only the
//! outgoing mail is a fake that keeps what it was asked to send.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use application::{
    DirectoryProbe, EnvSettings, HealthService, LdapSettings, MailSettings, Mailer,
    OAuthProviderSettings, OidcDiscovery, OutgoingMail, SecretCipher, SettingsConnectors,
    SettingsService, SettingsStore, SettingsTargets, StoredSection,
};
use async_trait::async_trait;
use auth_gateway::{AuditService, OAuthLoginService, SessionService};
use auth_gateway::{BootstrapService, LoginService, PasswordResetService};
use auth_gateway::{
    DirectoryAuthenticator, DirectoryError, DirectoryUser, OAuthProvider, PasswordPolicy,
};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::{
    Argon2Hasher, Argon2Params, OAuthSecrets, PgAdminRepository, PgAuditRepository,
    PgDirectoryAccounts, PgHealthCheck, PgIpLockoutStore, PgOAuthIdentityRepository,
    PgPasswordResetStore, PgPendingAuthStore, PgSessionRepository, PgSettingsStore,
    ResetTokenAdapter, SecretBox, SecureRandomTokens, SystemClock,
};
use serde_json::{json, Value};
use tower::ServiceExt;

const ADMIN_EMAIL: &str = "admin@example.com";
const ADMIN_PASSWORD: &str = "Str0ngEnoughPassphrase!";
const ANA: &str = "ana@empresa.com";
const ANA_PASSWORD: &str = "Ana-Elige-Su-Clave-1";

#[derive(Default)]
struct Outbox(Mutex<Vec<OutgoingMail>>);

#[async_trait]
impl Mailer for Outbox {
    async fn send(&self, mail: &OutgoingMail) -> Result<(), String> {
        self.0.lock().unwrap().push(mail.clone());
        Ok(())
    }
}

struct Store(PgSettingsStore);

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
    async fn save(&self, name: &str, data: Value, by: &str) -> Result<StoredSection, String> {
        self.0.save(name, data, by).await.map(section)
    }
    async fn delete(&self, name: &str) -> Result<(), String> {
        self.0.delete(name).await
    }
}

struct Cipher(SecretBox);

impl SecretCipher for Cipher {
    fn encrypt(&self, plain: &str) -> Result<String, String> {
        self.0.encrypt(plain)
    }
    fn decrypt(&self, sealed: &str) -> Result<String, String> {
        self.0.decrypt(sealed)
    }
}

struct NoDirectory;

#[async_trait]
impl DirectoryAuthenticator for NoDirectory {
    async fn authenticate(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Option<DirectoryUser>, DirectoryError> {
        Ok(None)
    }
}

struct NoConnectors;

#[async_trait]
impl SettingsConnectors for NoConnectors {
    fn directory(&self, _: &LdapSettings) -> Result<Arc<dyn DirectoryAuthenticator>, String> {
        Ok(Arc::new(NoDirectory))
    }
    async fn probe_directory(
        &self,
        _: &LdapSettings,
        _: Option<&str>,
    ) -> Result<DirectoryProbe, String> {
        Err("not in this test".into())
    }
    fn oauth_provider(&self, _: &OAuthProviderSettings) -> Result<Arc<dyn OAuthProvider>, String> {
        Err("not in this test".into())
    }
    async fn discover_oidc(&self, _: &str) -> Result<OidcDiscovery, String> {
        Err("not in this test".into())
    }
    fn mailer(&self, _: &MailSettings) -> Result<Arc<dyn Mailer>, String> {
        Err("not in this test".into())
    }
}

fn hasher() -> Arc<Argon2Hasher> {
    Arc::new(Argon2Hasher::new(Argon2Params::owasp_default()).unwrap())
}

/// The app, with outgoing mail going to `outbox` (`None`: no mail).
async fn app(pool: sqlx::PgPool, outbox: Option<Arc<Outbox>>) -> axum::Router {
    let admins = Arc::new(PgAdminRepository::new(pool.clone()));
    BootstrapService::new(admins.clone(), hasher(), PasswordPolicy::recommended())
        .create_first_admin(ADMIN_EMAIL, ADMIN_PASSWORD)
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
    let engine_repo = Arc::new(infrastructure::PgPlatformRepository::new(pool.clone()));
    let key_file = std::env::temp_dir()
        .join(format!(
            "orq-users-it-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
        .join("settings.key");
    let settings = SettingsService::new(
        Arc::new(Store(PgSettingsStore::new(pool.clone()))),
        Arc::new(Cipher(SecretBox::from_env_or_file(&key_file).unwrap())),
        Arc::new(NoConnectors),
        SettingsTargets {
            directory: login.directory_slot(),
            directory_accounts: Arc::new(PgDirectoryAccounts::new(pool.clone())),
            oauth: OAuthLoginService::new(
                Vec::new(),
                Arc::new(PgPendingAuthStore::new(pool.clone())),
                Arc::new(PgOAuthIdentityRepository::new(pool.clone())),
                admins.clone(),
                Arc::new(OAuthSecrets),
                Arc::new(SystemClock),
                "https://orq.example.com/api",
            ),
        },
        EnvSettings::default(),
    );
    settings.init().await.unwrap();
    // The platform's mail, as if no company mail were configured.
    if let Some(outbox) = outbox {
        settings.mailer().set_platform(Some(outbox));
    }
    let reset = PasswordResetService::new(
        admins.clone(),
        Arc::new(PgPasswordResetStore::new(pool.clone())),
        Arc::new(ResetTokenAdapter),
        hasher(),
        Arc::new(SystemClock),
        settings.mailer(),
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
        application::PlatformService::new(engine_repo),
        Some(settings),
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

/// A response's status and JSON body (`Null` when empty).
async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    auth: Option<&Client>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-forwarded-for", "203.0.113.5")
        .header("content-type", "application/json");
    if let Some(c) = auth {
        req = req
            .header("cookie", format!("session={}; csrf={}", c.session, c.csrf))
            .header("x-csrf-token", &c.csrf);
    }
    let response = app
        .clone()
        .oneshot(
            req.body(body.map_or(Body::empty(), |b| Body::from(b.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A signed-in browser: its session cookie and CSRF token.
struct Client {
    session: String,
    csrf: String,
}

async fn sign_in(app: &axum::Router, email: &str, password: &str) -> Result<Client, StatusCode> {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("x-forwarded-for", "203.0.113.5")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "email": email, "password": password }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    if response.status() != StatusCode::OK {
        return Err(response.status());
    }
    let cookie = |name: &str| {
        response
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|c| c.strip_prefix(&format!("{name}=")))
            .and_then(|c| c.split(';').next())
            .unwrap()
            .to_string()
    };
    Ok(Client {
        session: cookie("session"),
        csrf: cookie("csrf"),
    })
}

fn user<'a>(list: &'a Value, email: &str) -> &'a Value {
    list["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["email"] == email)
        .unwrap_or_else(|| panic!("{email} not in {list}"))
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // TODO(0034): split
async fn a_created_user_activates_with_the_emailed_link_and_signs_in() {
    let db = testkit::spawn_test_db().await;
    let outbox = Arc::new(Outbox::default());
    let app = app(db.pool.clone(), Some(outbox.clone())).await;
    let admin = sign_in(&app, ADMIN_EMAIL, ADMIN_PASSWORD).await.unwrap();

    // 1. The admin creates Ana.
    let (status, body) = call(
        &app,
        "POST",
        "/users",
        Some(&admin),
        Some(json!({ "email": "Ana@Empresa.com", "display_name": "Ana Ruiz", "role": "user" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["activation_sent"], true);
    let ana_id = body["id"].as_str().unwrap().to_string();

    // Same email again: refused.
    let (status, _) = call(
        &app,
        "POST",
        "/users",
        Some(&admin),
        Some(json!({ "email": ANA, "role": "user" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // 2. She is listed, pending.
    let (status, list) = call(&app, "GET", "/users", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(user(&list, ANA)["status"], "pending");
    assert_eq!(user(&list, ANA)["display_name"], "Ana Ruiz");
    assert_eq!(user(&list, ADMIN_EMAIL)["status"], "active");
    assert_eq!(list["mail_configured"], true);
    assert_eq!(list["roles"], json!(["admin", "user"]));

    // 3. Until she activates, no password opens the account (401, not 500).
    for password in ["!pending", ANA_PASSWORD] {
        assert_eq!(
            sign_in(&app, ANA, password).await.err(),
            Some(StatusCode::UNAUTHORIZED)
        );
    }

    // 4. The email carries the activation link, which sets her password.
    let mail = outbox.0.lock().unwrap().clone();
    assert_eq!(mail.len(), 1);
    assert_eq!(mail[0].to, vec![ANA.to_string()]);
    assert!(mail[0].text.contains("Hola, Ana Ruiz:"), "{}", mail[0].text);
    let token = mail[0]
        .text
        .split_whitespace()
        .find_map(|w| w.strip_prefix("https://orq.example.com/activate#"))
        .expect("the mail carries the link")
        .to_string();
    let (status, body) = call(
        &app,
        "POST",
        "/auth/password-reset/check",
        None,
        Some(json!({ "token": token })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["valid"], true);
    let (status, _) = call(
        &app,
        "POST",
        "/auth/password-reset/confirm",
        None,
        Some(json!({ "token": token, "password": ANA_PASSWORD })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let ana = sign_in(&app, ANA, ANA_PASSWORD).await.unwrap();

    // 5. Now active: nothing to send again. And she is no admin.
    let (_, list) = call(&app, "GET", "/users", Some(&admin), None).await;
    assert_eq!(user(&list, ANA)["status"], "active");
    let (status, _) = call(
        &app,
        "POST",
        &format!("/users/{ana_id}/activation"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call(&app, "GET", "/users", Some(&ana), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // 6. Made admin, her open session can manage users at once; then back.
    let (status, _) = call(
        &app,
        "PATCH",
        &format!("/users/{ana_id}"),
        Some(&admin),
        Some(json!({ "display_name": "Ana R.", "role": "admin" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, list) = call(&app, "GET", "/users", Some(&ana), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(user(&list, ANA)["role"], "admin");
    assert_eq!(user(&list, ANA)["display_name"], "Ana R.");
    let (status, _) = call(
        &app,
        "PATCH",
        &format!("/users/{ana_id}"),
        Some(&ana),
        Some(json!({ "role": "user" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "nobody changes their own role"
    );
    let (status, _) = call(
        &app,
        "PATCH",
        &format!("/users/{ana_id}"),
        Some(&admin),
        Some(json!({ "role": "user" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(&app, "GET", "/users", Some(&ana), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // 7. The admin cannot delete themselves (nor the only admin); Ana goes.
    let (_, list) = call(&app, "GET", "/users", Some(&admin), None).await;
    let admin_id = user(&list, ADMIN_EMAIL)["id"].as_str().unwrap().to_string();
    let (status, _) = call(
        &app,
        "DELETE",
        &format!("/users/{admin_id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call(
        &app,
        "DELETE",
        &format!("/users/{ana_id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        sign_in(&app, ANA, ANA_PASSWORD).await.err(),
        Some(StatusCode::UNAUTHORIZED)
    );

    let events: Vec<String> =
        sqlx::query_scalar("SELECT event_type FROM admin_audit_events WHERE email_attempted = $1")
            .bind(ANA)
            .fetch_all(&db.pool)
            .await
            .unwrap();
    assert!(events.contains(&"user_created".to_string()), "{events:?}");
    assert!(events.contains(&"user_updated".to_string()), "{events:?}");
    assert!(events.contains(&"user_deleted".to_string()), "{events:?}");
}

#[tokio::test]
async fn a_pending_user_can_get_the_link_again_and_only_the_last_one_works() {
    let db = testkit::spawn_test_db().await;
    let outbox = Arc::new(Outbox::default());
    let app = app(db.pool.clone(), Some(outbox.clone())).await;
    let admin = sign_in(&app, ADMIN_EMAIL, ADMIN_PASSWORD).await.unwrap();

    let (_, body) = call(
        &app,
        "POST",
        "/users",
        Some(&admin),
        Some(json!({ "email": ANA, "role": "admin" })),
    )
    .await;
    let id = body["id"].as_str().unwrap().to_string();
    let (status, _) = call(
        &app,
        "POST",
        &format!("/users/{id}/activation"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let tokens: Vec<String> = outbox
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|m| {
            m.text
                .split_whitespace()
                .find_map(|w| w.strip_prefix("https://orq.example.com/activate#"))
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(tokens.len(), 2);
    for (token, valid) in [(&tokens[0], false), (&tokens[1], true)] {
        let (_, body) = call(
            &app,
            "POST",
            "/auth/password-reset/check",
            None,
            Some(json!({ "token": token })),
        )
        .await;
        assert_eq!(body["valid"], valid);
    }
}

#[tokio::test]
async fn without_outgoing_mail_no_user_is_created() {
    let db = testkit::spawn_test_db().await;
    let app = app(db.pool.clone(), None).await;
    let admin = sign_in(&app, ADMIN_EMAIL, ADMIN_PASSWORD).await.unwrap();

    let (status, _) = call(
        &app,
        "POST",
        "/users",
        Some(&admin),
        Some(json!({ "email": ANA, "role": "user" })),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (_, list) = call(&app, "GET", "/users", Some(&admin), None).await;
    assert_eq!(list["users"].as_array().unwrap().len(), 1);
    assert_eq!(list["mail_configured"], false);
}

#[tokio::test]
async fn only_admin_and_user_are_roles_and_only_an_admin_makes_admins() {
    let db = testkit::spawn_test_db().await;
    let outbox = Arc::new(Outbox::default());
    let app = app(db.pool.clone(), Some(outbox)).await;
    let admin = sign_in(&app, ADMIN_EMAIL, ADMIN_PASSWORD).await.unwrap();

    // The permissions are policies (ADR 0016): no other role exists.
    let (_, list) = call(&app, "GET", "/users", Some(&admin), None).await;
    assert_eq!(list["roles"], json!(["admin", "user"]));
    let (status, _) = call(
        &app,
        "POST",
        "/users",
        Some(&admin),
        Some(json!({ "email": ANA, "role": "auditor" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Whoever manages users by a policy cannot make an admin.
    let id: String = sqlx::query_scalar(
        "INSERT INTO admin_users (email, password_hash, role)
         SELECT 'hr@empresa.com', password_hash, 'user' FROM admin_users WHERE email = $1
         RETURNING id::text",
    )
    .bind(ADMIN_EMAIL)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO iam_policies (name, statements)
         VALUES ('people', '[{\"effect\":\"Allow\",\"actions\":[\"admin:ManageUsers\"],\"resources\":[\"*\"]}]')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO iam_attachments (principal_kind, principal_id, policy)
         VALUES ('user', $1::uuid, 'people')",
    )
    .bind(&id)
    .execute(&db.pool)
    .await
    .unwrap();
    let hr = sign_in(&app, "hr@empresa.com", ADMIN_PASSWORD)
        .await
        .unwrap();
    let (status, _) = call(&app, "GET", "/users", Some(&hr), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(
        &app,
        "POST",
        "/users",
        Some(&hr),
        Some(json!({ "email": ANA, "role": "admin" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "admin_only");
    let (status, body) = call(
        &app,
        "POST",
        "/users",
        Some(&hr),
        Some(json!({ "email": ANA, "role": "user" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}
