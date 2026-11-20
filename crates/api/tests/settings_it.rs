//! End-to-end system settings: the real router, with session, CSRF and RBAC,
//! against an ephemeral, freshly migrated Postgres. It checks that only an
//! admin sees them, that they are validated per field, that secrets are stored
//! encrypted and never leave in a response, and that going back to the
//! environment deletes what was stored.
//!
//! The connectors are fakes (there is no LDAP or SMTP in the test): what is
//! tested here is the HTTP → service → Postgres path.

use std::sync::Arc;
use std::time::Duration;

use application::{
    DirectoryProbe, EnvSettings, HealthService, LdapSettings, MailSettings, Mailer,
    OAuthProviderSettings, OidcDiscovery, SecretCipher, SettingsConnectors, SettingsService,
    SettingsStore, SettingsTargets, StoredSection,
};
use async_trait::async_trait;
use auth_gateway::{
    AdminRepository, DirectoryAuthenticator, DirectoryError, DirectoryUser, NewAdmin,
    OAuthProvider, PasswordHasher,
};
use auth_gateway::{AuditService, OAuthLoginService, SessionService};
use auth_gateway::{BootstrapService, LoginService};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain::{Email, Role};
use infrastructure::{
    Argon2Hasher, Argon2Params, OAuthSecrets, PgAdminRepository, PgAuditRepository,
    PgDirectoryAccounts, PgHealthCheck, PgIpLockoutStore, PgOAuthIdentityRepository,
    PgPendingAuthStore, PgSessionRepository, PgSettingsStore, SecretBox, SecureRandomTokens,
    SystemClock,
};
use serde_json::{json, Value};
use tower::ServiceExt;

const ADMIN_EMAIL: &str = "admin@example.com";
const ADMIN_PASSWORD: &str = "Str0ngEnoughPassphrase!";
const USER_EMAIL: &str = "user@example.com";
const USER_PASSWORD: &str = "AlsoStr0ngEnoughPassphrase!";

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

struct FakeConnectors;

#[async_trait]
impl SettingsConnectors for FakeConnectors {
    fn directory(&self, _: &LdapSettings) -> Result<Arc<dyn DirectoryAuthenticator>, String> {
        Ok(Arc::new(NoDirectory))
    }
    async fn probe_directory(
        &self,
        _: &LdapSettings,
        _: Option<&str>,
    ) -> Result<DirectoryProbe, String> {
        Err("cannot connect to ldaps://dc.example.com: connection refused".into())
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

async fn router(pool: sqlx::PgPool) -> axum::Router {
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
    let engine_repo = Arc::new(infrastructure::PgPlatformRepository::new(pool.clone()));
    let oauth = OAuthLoginService::new(
        Vec::new(),
        Arc::new(PgPendingAuthStore::new(pool.clone())),
        Arc::new(PgOAuthIdentityRepository::new(pool.clone())),
        Arc::new(PgAdminRepository::new(pool.clone())),
        Arc::new(OAuthSecrets),
        Arc::new(SystemClock),
        "https://orq.example.com/api",
    );
    let key_file = std::env::temp_dir()
        .join(format!(
            "orq-settings-it-{}-{}",
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
        Arc::new(FakeConnectors),
        SettingsTargets {
            directory: login.directory_slot(),
            directory_accounts: Arc::new(PgDirectoryAccounts::new(pool.clone())),
            oauth,
        },
        EnvSettings::default(),
    );
    settings.init().await.unwrap();
    api::router(
        HealthService::new(Arc::new(PgHealthCheck::new(pool.clone()))),
        login,
        sessions,
        AuditService::new(Arc::new(PgAuditRepository::new(pool.clone()))),
        Arc::new(PgAdminRepository::new(pool.clone())),
        None,
        None,
        None,
        application::PlatformService::new(engine_repo),
        Some(settings),
        None,
        &[],
        api::rate_limit::RateLimitConfig {
            max_requests: 100,
            window: Duration::from_secs(60),
        },
    )
}

async fn seed(pool: sqlx::PgPool) {
    let hasher = Arc::new(Argon2Hasher::new(Argon2Params::owasp_default()).unwrap());
    let repo = Arc::new(PgAdminRepository::new(pool));
    BootstrapService::new(
        repo.clone(),
        hasher.clone(),
        auth_gateway::PasswordPolicy::recommended(),
    )
    .create_first_admin(ADMIN_EMAIL, ADMIN_PASSWORD)
    .await
    .unwrap();
    repo.insert(&NewAdmin {
        email: Email::parse(USER_EMAIL).unwrap(),
        password_hash: hasher.hash(USER_PASSWORD).await.unwrap(),
        role: Role::parse("user").unwrap(),
    })
    .await
    .unwrap();
}

/// The `session` cookie and the CSRF token of a sign-in.
struct Client {
    app: axum::Router,
    session: String,
    csrf: String,
}

fn cookie(response: &axum::http::Response<Body>, name: &str) -> String {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .find_map(|v| {
            let (pair, _) = v.to_str().ok()?.split_once(';')?;
            let (k, v) = pair.split_once('=')?;
            (k == name).then(|| v.to_string())
        })
        .unwrap_or_else(|| panic!("no {name} cookie"))
}

impl Client {
    async fn login(app: &axum::Router, email: &str, password: &str) -> Self {
        let response = app
            .clone()
            .oneshot(
                Request::post("/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({ "email": email, "password": password }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        Self {
            app: app.clone(),
            session: cookie(&response, "session"),
            csrf: cookie(&response, "csrf"),
        }
    }

    async fn call(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
        csrf: bool,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(uri).header(
            "cookie",
            format!("session={}; csrf={}", self.session, self.csrf),
        );
        if csrf {
            request = request.header("x-csrf-token", &self.csrf);
        }
        let request = match body {
            Some(b) => request
                .header("content-type", "application/json")
                .body(Body::from(b.to_string())),
            None => request.body(Body::empty()),
        }
        .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

fn ldap(password: &str) -> Value {
    json!({
        "enabled": true,
        "url": "ldaps://dc.example.com:636",
        "bind_dn": "CN=svc,DC=example,DC=com",
        "bind_password": password,
        "base_dn": "DC=example,DC=com",
        "admin_groups": ["CN=Admins,DC=example,DC=com", "  "],
    })
}

#[tokio::test]
async fn only_an_admin_reads_or_changes_the_settings() {
    let db = testkit::spawn_test_db().await;
    seed(db.pool.clone()).await;
    let app = router(db.pool.clone()).await;

    let user = Client::login(&app, USER_EMAIL, USER_PASSWORD).await;
    let (status, _) = user.call("GET", "/settings", None, false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = user
        .call("PUT", "/settings/ldap", Some(ldap("x")), true)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let admin = Client::login(&app, ADMIN_EMAIL, ADMIN_PASSWORD).await;
    let (status, body) = admin.call("GET", "/settings", None, false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ldap"]["source"]["kind"], "unset");
    assert_eq!(body["encryption_key"], "file");
    assert_eq!(
        body["oauth_redirect_template"],
        "https://orq.example.com/api/auth/oauth/{id}/callback"
    );

    // A mutation without the CSRF header does not pass, even an admin's.
    let (status, _) = admin
        .call("PUT", "/settings/ldap", Some(ldap("x")), false)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn secrets_are_encrypted_at_rest_and_never_returned() {
    let db = testkit::spawn_test_db().await;
    seed(db.pool.clone()).await;
    let app = router(db.pool.clone()).await;
    let admin = Client::login(&app, ADMIN_EMAIL, ADMIN_PASSWORD).await;

    // Per-field validation.
    let (status, body) = admin
        .call(
            "PUT",
            "/settings/ldap",
            Some(json!({ "enabled": true, "url": "http://dc", "user_filter": "(mail=x)" })),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    for field in ["url", "bind_dn", "bind_password", "base_dn", "user_filter"] {
        assert!(body["fields"][field].is_string(), "missing {field}: {body}");
    }

    let (status, body) = admin
        .call("PUT", "/settings/ldap", Some(ldap("Sup3r-Secret")), true)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["value"]["bind_password"], "");
    assert_eq!(body["value"]["has_bind_password"], true);
    assert_eq!(
        body["value"]["admin_groups"],
        json!(["CN=Admins,DC=example,DC=com"])
    );
    assert_eq!(body["source"]["kind"], "database");
    assert_eq!(body["source"]["updated_by"], ADMIN_EMAIL);

    // In the database it is encrypted.
    let stored: Value =
        sqlx::query_scalar("SELECT data FROM system_settings WHERE section = 'ldap'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let sealed = stored["bind_password"].as_str().unwrap();
    assert!(sealed.starts_with("v1:"), "{sealed}");
    assert!(!stored.to_string().contains("Sup3r-Secret"));

    // Sending again without the password keeps it.
    let mut edit = ldap("");
    edit["base_dn"] = json!("OU=People,DC=example,DC=com");
    let (status, body) = admin.call("PUT", "/settings/ldap", Some(edit), true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["value"]["has_bind_password"], true);
    let (_, all) = admin.call("GET", "/settings", None, false).await;
    assert!(!all.to_string().contains("Sup3r-Secret"));
    assert_eq!(
        all["ldap"]["value"]["base_dn"],
        "OU=People,DC=example,DC=com"
    );

    // A test that does not connect answers 502 with the reason.
    let (status, body) = admin
        .call(
            "POST",
            "/settings/ldap/test",
            Some(json!({ "settings": ldap(""), "login": "ana@example.com" })),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("connection refused"));

    // Volver al entorno borra la fila.
    let (status, body) = admin.call("DELETE", "/settings/ldap", None, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ldap"]["source"]["kind"], "unset");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM system_settings")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn the_platform_saves_settings_without_an_admin() {
    let db = testkit::spawn_test_db().await;
    let store = PgSettingsStore::new(db.pool.clone());
    let row = store
        .save("mail", json!({"enabled": true, "platform": true}), "")
        .await
        .unwrap();
    assert_eq!(row.updated_by, None);
    assert_eq!(row.data["platform"], true);
}
