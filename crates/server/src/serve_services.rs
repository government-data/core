//! `server serve`'s services: sign-in, the platform, updates, sessions,
//! OAuth, the system settings and password recovery, built on the pool
//! before any route is.

use std::sync::Arc;

use api::internal::AdminAccessRoute;
use api::oauth::OAuthRedirects;
use api::password_reset::ResetLinkBase;
use application::{
    EnvSettings, HealthService, PlatformService, SettingsService, SettingsTargets, UpdatesService,
};
use auth_gateway::{AuditService, OAuthLoginService, SessionService};
use auth_gateway::{LoginService, PasswordResetService};
use infrastructure::auth_config::AuthConfig;
use infrastructure::{
    Argon2Hasher, OAuthSecrets, PgAdminRepository, PgAuditRepository, PgDirectoryAccounts,
    PgHealthCheck, PgIpLockoutStore, PgOAuthIdentityRepository, PgPasswordResetStore,
    PgPendingAuthStore, PgPlatformRepository, PgPool, PgSessionRepository, PgSettingsStore,
    ReqwestHttpClient, ResetTokenAdapter, SecretBox, SecureRandomTokens, SystemClock,
};

use crate::background::{spawn_background, spawn_scheduler};
use crate::config::{env_value, public_url_from_env, OAuthSettings};
use crate::tenants::Tenancy;
use crate::{settings_wiring, updates_wiring};

/// Where the settings key lives when `SETTINGS_ENCRYPTION_KEY` is not set (a
/// volume; it is generated the first time).
const DEFAULT_SETTINGS_KEY_FILE: &str = "/var/lib/quality-data/settings/settings.key";

type Failure = Box<dyn std::error::Error>;

/// What the routes are built from.
pub(crate) struct Services {
    pub(crate) health: HealthService,
    pub(crate) admins: Arc<dyn auth_gateway::AdminRepository>,
    pub(crate) login: LoginService,
    pub(crate) secret_box: SecretBox,
    pub(crate) platform: PlatformService,
    pub(crate) updates: UpdatesService,
    pub(crate) sessions: SessionService,
    pub(crate) audit: AuditService,
    pub(crate) oauth: Option<(OAuthLoginService, OAuthRedirects)>,
    pub(crate) settings: SettingsService,
    pub(crate) captcha: api::captcha::Captcha,
    pub(crate) password_reset: (PasswordResetService, ResetLinkBase),
    pub(crate) admin_access: Option<AdminAccessRoute>,
}

/// Where the settings key is read from: `SETTINGS_KEY_FILE`, or the default.
pub(crate) fn settings_key_file() -> String {
    env_value("SETTINGS_KEY_FILE").unwrap_or_else(|| DEFAULT_SETTINGS_KEY_FILE.into())
}

/// The services. The health probe reflects live database connectivity;
/// the login service verifies admin credentials with argon2id,
/// constant-time equalization, and progressive account/IP lockout.
pub(crate) async fn services(tenancy: &Tenancy, auth: &AuthConfig) -> Result<Services, Failure> {
    let pool = tenancy.pool.clone();
    let health = HealthService::new(Arc::new(PgHealthCheck::new(pool.clone())));
    let admins: Arc<dyn auth_gateway::AdminRepository> =
        Arc::new(PgAdminRepository::new(pool.clone()));
    let platform_repo: Arc<dyn platform::PlatformRepository> =
        Arc::new(PgPlatformRepository::new(pool.clone()));
    let login = LoginService::new(
        admins.clone(),
        Arc::new(PgIpLockoutStore::new(pool.clone())),
        Arc::new(Argon2Hasher::new(auth.argon2)?),
        Arc::new(SystemClock),
        auth.lockout_policy,
    );

    // The key that seals the settings' secrets and the global secrets: `SETTINGS_ENCRYPTION_KEY` or, without it, the file in
    // `SETTINGS_KEY_FILE`.
    let secret_box = SecretBox::from_env_or_file(settings_key_file())?;

    let platform = platform(&pool, platform_repo.clone(), &secret_box);
    // Update maintenance: while it lasts, no new work starts.
    let updates = UpdatesService::new(Arc::new(updates_wiring::Store(
        infrastructure::PgMaintenanceStore::new(pool.clone()),
    )));
    let platform = platform.with_updates(updates.clone());
    let sessions = SessionService::new(
        Arc::new(PgSessionRepository::new(pool.clone())),
        Arc::new(SecureRandomTokens),
        Arc::new(SystemClock),
        auth.session_policy,
    )
    // Service accounts call the API with their token (ADR 0016).
    .with_service_tokens(Arc::new(infrastructure::PgServiceTokens::new(pool.clone())));
    let audit = AuditService::new(Arc::new(PgAuditRepository::new(pool.clone())));

    let (oauth_service, redirects, settings) = settings(&pool, auth, &secret_box, &login).await?;
    let (captcha, _) = settings_wiring::captcha_from_env()?;
    spawn_rounds(&settings, tenancy);
    // Every new version and how each attempt ended, emailed to the
    // administrators (the daemon reports it through the internal routes).
    let updates = updates.with_notices(
        settings.mailer(),
        Arc::new(application::PlatformAdmins(platform_repo)),
    );
    tracing::info!(
        ldap = settings.ldap().value.enabled,
        mail = settings.mail().value.enabled,
        oauth_providers = oauth_service.provider_ids().len(),
        key = ?secret_box.source(),
        "system settings applied"
    );
    // The security notices to each account (it was locked, a provider was
    // linked or unlinked) go out with the settings' mail.
    let login = login.with_mail(settings.mailer());
    let oauth_service = oauth_service.with_mail(settings.mailer());

    let (password_reset, reset_links, admin_access) =
        password_reset(&pool, &admins, auth, &settings, &audit)?;
    Ok(Services {
        health,
        admins,
        login,
        secret_box,
        platform,
        updates,
        sessions,
        audit,
        oauth: Some((oauth_service, redirects)),
        settings,
        captcha,
        password_reset: (password_reset, reset_links),
        admin_access,
    })
}

/// The background round (the platform mail), once the settings are loaded,
/// and the schedules (decision 0035): only the VM holding the lock fires.
fn spawn_rounds(settings: &SettingsService, tenancy: &Tenancy) {
    spawn_background(settings.clone());
    spawn_scheduler(tenancy.pool.clone());
}

/// The platform: the actor and IAM, secrets and updates.
fn platform(
    pool: &PgPool,
    platform_repo: Arc<dyn platform::PlatformRepository>,
    secret_box: &SecretBox,
) -> PlatformService {
    let platform = PlatformService::new(platform_repo)
        .with_cipher(Arc::new(settings_wiring::Cipher(secret_box.clone())))
        .with_tokens(Arc::new(infrastructure::ServiceTokenMint));
    // When what someone can do changes (here, in another replica or directly
    // in the database), the Actor kept in memory stops being valid (ADR 0013).
    {
        let platform = platform.clone();
        infrastructure::permission_notices::spawn_listener(pool.clone(), move |tenant| {
            platform.invalidate_permissions(tenant)
        });
    }
    platform
}

/// OAuth and the system settings. The OAuth routes are always mounted (with
/// no providers they list none and answer `404`), so providers added in the
/// settings work at once. The generic OIDC adapter serves every provider off
/// one shared HTTP client. The settings (LDAP, mail, OAuth) are edited from
/// the UI and applied live; what was never stored takes `LDAP_*` /
/// `OAUTH_*` from the environment.
pub(crate) async fn settings(
    pool: &PgPool,
    auth: &AuthConfig,
    secret_box: &SecretBox,
    login: &LoginService,
) -> Result<(OAuthLoginService, OAuthRedirects, SettingsService), Failure> {
    let http: Arc<dyn infrastructure::HttpClient> = Arc::new(ReqwestHttpClient::new());
    let env_oauth = OAuthSettings::from_env()?;
    let (redirect_base, redirects) = OAuthSettings::redirects_from_env();
    let oauth_service = OAuthLoginService::new(
        Vec::new(),
        Arc::new(PgPendingAuthStore::new(pool.clone())),
        Arc::new(PgOAuthIdentityRepository::new(pool.clone())),
        Arc::new(PgAdminRepository::new(pool.clone())),
        Arc::new(OAuthSecrets),
        Arc::new(SystemClock),
        redirect_base,
    );
    let settings = SettingsService::new(
        Arc::new(settings_wiring::Store(PgSettingsStore::new(pool.clone()))),
        Arc::new(settings_wiring::Cipher(secret_box.clone())),
        Arc::new(settings_wiring::Connectors { http: http.clone() }),
        SettingsTargets {
            directory: login.directory_slot(),
            directory_accounts: Arc::new(PgDirectoryAccounts::new(pool.clone())),
            oauth: oauth_service.clone(),
        },
        EnvSettings {
            ldap: settings_wiring::ldap_from_env()?,
            oauth: env_oauth
                .as_ref()
                .and_then(|o| settings_wiring::oauth_from_env(&o.providers)),
            reset_link_minutes: auth.password_reset_ttl.as_secs() / 60,
        },
    )
    .with_key_source(match secret_box.source() {
        infrastructure::KeySource::Environment => "environment",
        infrastructure::KeySource::File => "file",
        infrastructure::KeySource::Generated => "generated",
    });
    settings.init().await?;
    Ok((oauth_service, redirects, settings))
}

/// Password recovery: a link by email (the settings' mail) that expires
/// after `PASSWORD_RESET_TTL_MINUTES`, pointing to the configured public
/// address. With it, the administrator's entry from the server (internal
/// routes).
pub(crate) fn password_reset(
    pool: &PgPool,
    admins: &Arc<dyn auth_gateway::AdminRepository>,
    auth: &AuthConfig,
    settings: &SettingsService,
    audit: &AuditService,
) -> Result<
    (
        PasswordResetService,
        ResetLinkBase,
        Option<AdminAccessRoute>,
    ),
    Failure,
> {
    let password_reset = PasswordResetService::new(
        admins.clone(),
        Arc::new(PgPasswordResetStore::new(pool.clone())),
        Arc::new(ResetTokenAdapter),
        Arc::new(Argon2Hasher::new(auth.argon2)?),
        Arc::new(SystemClock),
        settings.mailer(),
        auth.password_policy,
        auth.password_reset_ttl,
    );
    let reset_links = ResetLinkBase::Fixed(public_url_from_env());
    let admin_access = Some(AdminAccessRoute {
        service: password_reset.clone(),
        audit: audit.clone(),
        link_base: public_url_from_env(),
    });
    Ok((password_reset, reset_links, admin_access))
}
