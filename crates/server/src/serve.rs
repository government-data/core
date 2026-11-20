//! `server serve`: wire every layer and serve HTTP.
//!
//! 1. Load configuration from the environment.
//! 2. Register the feature modules.
//! 3. Connect the database pool and run migrations (startup schema check),
//!    then apply each module's own migrations.
//! 4. Initialize the modules.
//! 5. Build the router: health, sign-in, the platform, settings,
//!    and each module's routes, behind the tenant, license and language
//!    layers.
//! 6. Start the HTTP server (and the internal routes, on their own port).

use std::net::SocketAddr;
use std::sync::Arc;

use application::{
    Edition, EnvSettings, HealthService, PlatformService, SettingsService, SettingsTargets,
};
use auth_gateway::{AuditService, OAuthLoginService, SessionService};
use auth_gateway::{LoginService, PasswordResetService};
use infrastructure::auth_config::AuthConfig;
use infrastructure::{
    Argon2Hasher, OAuthSecrets, PgAdminRepository, PgAuditRepository, PgConfig,
    PgDirectoryAccounts, PgHealthCheck, PgIpLockoutStore, PgOAuthIdentityRepository,
    PgPasswordResetStore, PgPendingAuthStore, PgPlatformRepository, PgSessionRepository,
    PgSettingsStore, ReqwestHttpClient, ResetTokenAdapter, SecretBox, SecureRandomTokens,
    SystemClock,
};

use crate::background::{
    spawn_background, spawn_license_renewal, spawn_platform_mail, spawn_scheduler,
};
use crate::config::{env_value, public_url_from_env, Config, OAuthSettings};
use crate::startup::{local_license, oidc_provider};
use crate::updates_wiring;
use crate::{init_tracing, machines_wiring, settings_wiring, tenants};

/// Where the settings key lives when `SETTINGS_ENCRYPTION_KEY` is not set (a
/// volume; it is generated the first time).
const DEFAULT_SETTINGS_KEY_FILE: &str = "/var/lib/orquestador/settings/settings.key";

/// Run the HTTP server (the default command).
#[allow(clippy::too_many_lines)] // TODO(0034): split
pub(crate) async fn serve() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();

    // 1. Configuration from the environment.
    let config = Config::from_env()?;
    let auth = AuthConfig::from_env()?;
    let pg_config = PgConfig::from_env()?;
    let edition = Edition::parse(&std::env::var("ORQ_EDITION").unwrap_or_default())
        .map_err(|e| format!("ORQ_EDITION {e}"))?;

    // 2. Register the feature modules. Adding a feature is ONE line here; its
    // crate carries everything else (routes, migrations, init). Removing a
    // feature is the reverse and never touches another module.
    let modules = crate::modules::registry();

    // 3. The pool, the schema and how each request finds its tenant.
    let tenancy = tenants::connect(edition, &pg_config).await?;
    let pool = tenancy.pool.clone();

    // Apply every module's migrations — isolated per module and ordered — on
    // PostgreSQL, refusing to start if one already applied has changed.
    let report = crate::modules::migrate(&modules, &pool).await?;
    tracing::info!(
        applied = report.applied.len(),
        already_present = report.skipped.len(),
        "module migrations complete"
    );

    // 4. Initialize the modules once their schema is in place.
    modules.init()?;

    // 5. The services. The health probe reflects live database connectivity;
    // the login service verifies admin credentials with argon2id,
    // constant-time equalization, and progressive account/IP lockout.
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

    // The key that seals the settings' secrets, the global secrets and the
    // license: `SETTINGS_ENCRYPTION_KEY` or, without it, the file in
    // `SETTINGS_KEY_FILE`.
    let secret_box = SecretBox::from_env_or_file(
        env_value("SETTINGS_KEY_FILE").unwrap_or_else(|| DEFAULT_SETTINGS_KEY_FILE.into()),
    )?;
    let key_source = secret_box.source();

    // The platform: the actor and IAM, secrets, license and updates.
    tracing::info!(?edition, "edition");
    let platform = PlatformService::new(platform_repo.clone())
        .with_edition(edition)
        .with_cipher(Arc::new(settings_wiring::Cipher(secret_box.clone())))
        .with_tokens(Arc::new(infrastructure::ServiceTokenMint));
    let license = local_license(edition, &pool, &secret_box, &platform_repo).await?;
    let platform = match &license {
        Some(license) => platform.with_license(license.clone()),
        None => platform,
    };
    // When what someone can do changes (here, in another replica or directly
    // in the database), the Actor kept in memory stops being valid (ADR 0013).
    {
        let platform = platform.clone();
        infrastructure::permission_notices::spawn_listener(pool.clone(), move |tenant| {
            platform.invalidate_permissions(tenant)
        });
    }
    // Update maintenance: while it lasts, no new work starts.
    let updates = application::UpdatesService::new(Arc::new(updates_wiring::Store(
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

    // OAuth: the routes are always mounted (with no providers they list none
    // and answer `404`), so providers added in the settings work at once. The
    // generic OIDC adapter serves every provider off one shared HTTP client.
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

    // The system settings (LDAP, mail, OAuth), edited from the UI and applied
    // live. What was never stored takes `LDAP_*` / `OAUTH_*` from the
    // environment.
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
    .with_key_source(match key_source {
        infrastructure::KeySource::Environment => "environment",
        infrastructure::KeySource::File => "file",
        infrastructure::KeySource::Generated => "generated",
    });
    settings.init().await?;
    let (captcha, captcha_from_env) = settings_wiring::captcha_from_env()?;
    // The platform mail is configured in management (the licensing cloud):
    // the online cell or an instance we manage (its key marked as managed)
    // asks for it with its key and it is set on each company instead of its
    // own (in the background round): there mail is not configured on the
    // instance. A customer's installation does not get it and configures its
    // own.
    match (env_value("ORQ_NUBE"), env_value("ORQ_LICENSE_KEY")) {
        (Some(cloud), Some(key)) => {
            let installation = env_value("ORQ_INSTALLATION_ID")
                .or_else(|| env_value("ORQ_DOMINIO"))
                .unwrap_or_else(|| "cell".into());
            spawn_platform_mail(
                settings.clone(),
                infrastructure::HttpLicenseAuthority::new(
                    &cloud,
                    infrastructure::license::compiled_public_key(),
                )?,
                key,
                installation,
                (!captcha_from_env).then(|| captcha.clone()),
            );
        }
        _ if matches!(edition, Edition::Online) => tracing::warn!(
            "online without ORQ_NUBE and ORQ_LICENSE_KEY: no platform mail, \
             each company has to configure its own"
        ),
        _ => {}
    }
    // The background round of every tenant (the platform mail), once the
    // settings are loaded.
    spawn_background(
        settings.clone(),
        tenancy.directory.clone(),
        tenancy.cell.clone(),
    );
    // The schedules (decision 0035): only the VM holding the lock fires.
    spawn_scheduler(
        pool.clone(),
        tenancy.directory.clone(),
        tenancy.cell.clone(),
    );
    // Every new version and how each attempt ended, emailed to the
    // administrators (the daemon reports it through the internal routes).
    let updates = updates.with_notices(
        settings.mailer(),
        Arc::new(application::PlatformAdmins(platform_repo.clone())),
    );
    if let Some(license) = license {
        spawn_license_renewal(license.with_reminders(
            settings.mailer(),
            Arc::new(application::PlatformAdmins(platform_repo)),
        ));
    }
    tracing::info!(
        ldap = settings.ldap().value.enabled,
        mail = settings.mail().value.enabled,
        oauth_providers = oauth_service.provider_ids().len(),
        key = ?key_source,
        "system settings applied"
    );
    // The security notices to each account (it was locked, a provider was
    // linked or unlinked) go out with the settings' mail.
    let login = login.with_mail(settings.mailer());
    let oauth_service = oauth_service.with_mail(settings.mailer());
    let oauth = Some((oauth_service, redirects));

    // Password recovery: a link by email (the settings' mail) that expires
    // after `PASSWORD_RESET_TTL_MINUTES`. On-premise the link points to the
    // configured public address; online, to the subdomain of the company
    // that asked for it (the tenant middleware already rejected any other).
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
    let reset_links = match edition {
        Edition::OnPremise => api::password_reset::ResetLinkBase::Fixed(public_url_from_env()),
        Edition::Online => api::password_reset::ResetLinkBase::FromHost,
    };
    // The administrator's entry from the server (internal routes): on-premise
    // only, where there is one company and a fixed public address.
    let admin_access = (edition == Edition::OnPremise).then(|| api::internal::AdminAccessRoute {
        service: password_reset.clone(),
        audit: audit.clone(),
        link_base: public_url_from_env(),
    });

    // Online, the common entrance (entrar.<domain>): the email and the
    // password, without the company's address.
    let central = match (&tenancy.directory, &tenancy.domain) {
        (Some(directory), Some(domain)) => Some(api::central::routes(
            auth_gateway::CentralLoginService::new(
                login.clone(),
                directory.clone(),
                Arc::new(infrastructure::PgLoginHandoffs::new(pool.clone())),
                admins.clone(),
                Arc::new(ResetTokenAdapter),
                Arc::new(SystemClock),
                tenancy.cell.clone(),
                domain.clone(),
            )
            .with_audit(audit.clone()),
            sessions.clone(),
            audit.clone(),
            auth.login_rate_limit,
        )),
        _ => None,
    };
    // The gateway as an OIDC provider (with OIDC_SIGNING_KEY): the
    // applications registered with `server oidc-client` let their users in
    // with this installation's accounts.
    let oidc_issuer = format!("{}/api", public_url_from_env().trim_end_matches('/'));
    let provider = infrastructure::oidc::provider_from_env(&pool, admins.clone(), &oidc_issuer)?;
    let certificates = machines_wiring::client_certificates_from_env()?;
    let oidc = oidc_provider(
        provider.clone(),
        sessions.clone(),
        certificates.clone(),
        edition,
    );
    let provider = provider.filter(|_| edition == Edition::OnPremise);
    // Who the session is, for every route that requires a capability: the
    // feature modules' too (theirs are merged outside `api::router`).
    let resolver = api::iam::resolver(platform.clone());
    let dolt = crate::catalog_wiring::DoltSnapshots::from_env()?;
    let machines = machines_wiring::service(
        &pool,
        provider.clone(),
        (&tenancy).into(),
        secret_box.clone(),
    )?;
    let machines = match &dolt {
        Some(d) => machines.with_dolt(d.clone()),
        None => machines,
    };
    let callers = machines_wiring::certificate_callers(&certificates, &machines);
    let modules = crate::modules::serving(
        &pool,
        provider,
        secret_box.clone(),
        (&tenancy).into(),
        callers,
        dolt,
    )?;
    // The machines: Settings → Execution, the install routes (their own
    // limit) and the route they join with (the login's).
    let machine_routes = machines_wiring::routes(
        machines.clone(),
        sessions.clone(),
        auth.login_rate_limit,
        auth.install_rate_limit,
    )?;
    let module_users = crate::callers::user_routes(
        &modules,
        sessions.clone(),
        Arc::new(PgAuditRepository::new(pool.clone())),
    );
    let app = modules
        .router(api::router(
            health,
            login,
            sessions,
            audit,
            admins,
            oauth,
            platform,
            Some(settings),
            Some((password_reset, reset_links)),
            config.cors_allowed_origins(),
            auth.login_rate_limit,
        ))
        .merge(module_users)
        .merge(machine_routes);
    let app = match central {
        Some(central) => app.merge(central),
        None => app,
    };
    let app = app.layer(axum::Extension(resolver));
    // This installation's applications, for the avatar's «Aplicaciones».
    let app = app.layer(axum::Extension(api::settings::LocalApps(Arc::new(
        infrastructure::oidc::PgOidcClients::new(pool.clone()),
    ))));
    let app = match oidc {
        Some(provider) => app.merge(provider),
        None => app,
    };
    // The captcha of the public routes that send mail or sign in.
    let app = app.layer(axum::Extension(captcha));
    // The language of each request's messages (Accept-Language).
    let app = app.layer(axum::middleware::from_fn(api::language::per_request));
    // Each request's company wraps everything, modules included.
    let app = app.layer(axum::middleware::from_fn_with_state(
        tenancy.resolver,
        api::tenancy::resolve,
    ));
    // Each request counted by its route's template, and its metrics file
    // (decision 0071), with the approved machines' gauge (0072).
    let metrics = crate::metrics_wiring::start(
        pool.clone(),
        tenancy.directory.clone(),
        tenancy.cell.clone(),
        machines,
    );
    let app = app.layer(axum::middleware::from_fn_with_state(
        metrics,
        http_kit::metrics::track,
    ));
    // Outside it, every request's id and span: the modules' routes too,
    // merged after `api::router` put its own (which then only passes on).
    let app = app.layer(axum::middleware::from_fn(
        http_kit::telemetry::request_context,
    ));

    crate::startup::internal_routes(updates, admin_access).await?;

    // 6. Serve. `into_make_service_with_connect_info` records each connection's
    // peer address so the login handler can fall back to it for per-IP lockout
    // when no forwarded header is present.
    let listener = tokio::net::TcpListener::bind(config.socket_addr()).await?;
    tracing::info!(addr = %listener.local_addr()?, "listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}
