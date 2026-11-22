//! `server serve`: wire every layer and serve HTTP.
//!
//! 1. Load configuration from the environment.
//! 2. Register the feature modules.
//! 3. Connect the database pool and run migrations (startup schema check),
//!    then apply each module's own migrations.
//! 4. Initialize the modules.
//! 5. Build the services ([`serve_services`]) and the router: health,
//!    sign-in, the platform, settings, and each module's routes, behind the
//!    tenant and language layers.
//! 6. Start the HTTP server (and the internal routes, on their own port).
//!
//! [`serve_services`]: crate::serve_services

use std::net::SocketAddr;
use std::sync::Arc;

use api::internal::AdminAccessRoute;
use application::{Edition, MachinesService};
use auth_gateway::{AdminRepository, AuditService, AuthorizationServer};
use auth_gateway::{LoginService, SessionService};
use axum::Router;
use contracts::ModuleRegistry;
use infrastructure::auth_config::AuthConfig;
use infrastructure::{PgAuditRepository, PgConfig, ResetTokenAdapter, SecretBox, SystemClock};

use crate::config::{public_url_from_env, Config};
use crate::serve_services::{services, Services};
use crate::startup::oidc_provider;
use crate::tenants::Tenancy;
use crate::{drain_wiring, init_tracing, machines_wiring, tenants};

type Failure = Box<dyn std::error::Error>;

/// Run the HTTP server (the default command).
pub(crate) async fn serve() -> Result<(), Failure> {
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

    // 5. The services and the routes.
    let services = services(&tenancy, &auth, edition).await?;
    let (app, updates, admin_access) = app(services, tenancy, &config, &auth, edition)?;

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

/// The routes on the services, and what the internal routes need: the
/// updates and the administrator's entry.
fn app(
    services: Services,
    tenancy: Tenancy,
    config: &Config,
    auth: &AuthConfig,
    edition: Edition,
) -> Result<
    (
        Router,
        application::UpdatesService,
        Option<AdminAccessRoute>,
    ),
    Failure,
> {
    let Services {
        health,
        admins,
        login,
        secret_box,
        platform,
        updates,
        sessions,
        audit,
        oauth,
        settings,
        captcha,
        password_reset,
        admin_access,
    } = services;
    let central = central(&tenancy, &login, &admins, &sessions, &audit, auth);
    // The gateway as an OIDC provider (with OIDC_SIGNING_KEY): the
    // applications registered with `server oidc-client` let their users in
    // with this installation's accounts.
    let oidc_issuer = format!("{}/api", public_url_from_env().trim_end_matches('/'));
    let provider =
        infrastructure::oidc::provider_from_env(&tenancy.pool, admins.clone(), &oidc_issuer)?;
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
    let (machines, modules) = machines_and_modules(&tenancy, provider, &secret_box)?;
    // What the update agent waits for: each module's own work, and each
    // installed application module's as the agent tells them (0003).
    let app_modules =
        drain_wiring::AppModules::from_env(updates.clone(), drain_wiring::ANSWER_WITHIN)?;
    let updates = updates
        .with_modules_work(Arc::new(drain_wiring::CodeModules::of(
            &modules,
            drain_wiring::ANSWER_WITHIN,
        )))
        .with_modules_work(Arc::new(app_modules));
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
        Arc::new(PgAuditRepository::new(tenancy.pool.clone())),
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
            Some(password_reset),
            config.cors_allowed_origins(),
            auth.login_rate_limit,
        ))
        .merge(module_users)
        .merge(machine_routes)
        // The update agent's protocol, with its certificate (decision 0003).
        .merge(api::updater_v1::routes(
            updates.clone(),
            machines.clone(),
            certificates.clone(),
        ));
    let app = match central {
        Some(central) => app.merge(central),
        None => app,
    };
    let app = app.layer(axum::Extension(resolver));
    Ok((
        layered(app, tenancy, oidc, captcha, machines),
        updates,
        admin_access,
    ))
}

/// Online, the common entrance (entrar.<domain>): the email and the
/// password, without the company's address.
fn central(
    tenancy: &Tenancy,
    login: &LoginService,
    admins: &Arc<dyn AdminRepository>,
    sessions: &SessionService,
    audit: &AuditService,
    auth: &AuthConfig,
) -> Option<Router> {
    let (Some(directory), Some(domain)) = (&tenancy.directory, &tenancy.domain) else {
        return None;
    };
    Some(api::central::routes(
        auth_gateway::CentralLoginService::new(
            login.clone(),
            directory.clone(),
            Arc::new(infrastructure::PgLoginHandoffs::new(tenancy.pool.clone())),
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
    ))
}

/// The machines (with the catalog, `DOLT_URL`) and the feature modules with
/// the services their routes use.
fn machines_and_modules(
    tenancy: &Tenancy,
    provider: Option<AuthorizationServer>,
    secret_box: &SecretBox,
) -> Result<(MachinesService, ModuleRegistry), Failure> {
    let dolt = crate::catalog_wiring::DoltSnapshots::from_env()?;
    let machines = machines_wiring::service(
        &tenancy.pool,
        provider.clone(),
        tenancy.into(),
        secret_box.clone(),
    )?;
    let machines = match &dolt {
        Some(d) => machines.with_dolt(d.clone()),
        None => machines,
    };
    let modules = crate::modules::serving(&tenancy.pool, provider, secret_box.clone(), dolt)?;
    Ok((machines, modules))
}

/// The layers around every route, modules included.
fn layered(
    app: Router,
    tenancy: Tenancy,
    oidc: Option<Router>,
    captcha: api::captcha::Captcha,
    machines: MachinesService,
) -> Router {
    // This installation's applications, for the avatar's «Aplicaciones».
    let app = app.layer(axum::Extension(api::settings::LocalApps(Arc::new(
        infrastructure::oidc::PgOidcClients::new(tenancy.pool.clone()),
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
        tenancy.pool.clone(),
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
    app.layer(axum::middleware::from_fn(
        http_kit::telemetry::request_context,
    ))
}
