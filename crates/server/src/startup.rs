//! What `serve` builds from the environment before mounting it: the local
//! installation's license and the OIDC provider.

use std::sync::Arc;

use application::{Edition, LicenseService};
use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::SessionService;
use infrastructure::SecretBox;

use crate::config::env_value;

/// The local installation's license: only on-premise, with the licensing
/// cloud configured (`ORQ_NUBE`) and the identifier the installer left. Its
/// daily renewal starts once the settings' mail is loaded, to warn about the
/// expiry.
pub(crate) async fn local_license(
    edition: Edition,
    pool: &infrastructure::PgPool,
    secret_box: &SecretBox,
    usage: &Arc<dyn platform::PlatformRepository>,
) -> Result<Option<LicenseService>, Box<dyn std::error::Error>> {
    let (Edition::OnPremise, Some(cloud), Some(installation)) = (
        edition,
        env_value("ORQ_NUBE"),
        env_value("ORQ_INSTALLATION_ID"),
    ) else {
        return Ok(None);
    };
    let license = LicenseService::new(
        Arc::new(infrastructure::HttpLicenseAuthority::new(
            &cloud,
            infrastructure::license::compiled_public_key(),
        )?),
        Arc::new(infrastructure::PgLicenseStore::new(
            pool.clone(),
            Arc::new(secret_box.clone()),
        )),
        installation,
        env_value("ORQ_LICENSE_KEY"),
        infrastructure::license::uses_dev_key(),
    )
    .with_usage(usage.clone());
    if infrastructure::license::uses_dev_key() {
        tracing::warn!("licenses verified with the DEVELOPMENT key: not fit for customers");
    }
    license.load().await?;
    Ok(Some(license))
}

/// The OIDC provider, if there is a key (`OIDC_SIGNING_KEY`) and it is
/// on-premise. The issuer is `OIDC_ISSUER` or, without it, the public address
/// with `/api` (Traefik strips it).
/// `certificates` says whether its token endpoint reads a machine's
/// certificate (`INTERNAL_CLIENT_CERT_HEADER`, decision 0066).
pub(crate) fn oidc_provider(
    server: Option<auth_gateway::AuthorizationServer>,
    sessions: SessionService,
    certificates: ClientCertificates,
    edition: Edition,
) -> Option<axum::Router> {
    let server = server?;
    // Online each company has its subdomain and a fixed issuer would not be
    // its own: there the provider is the shared account's (`account`).
    if edition != Edition::OnPremise {
        tracing::warn!("OIDC_SIGNING_KEY ignored: the OIDC provider is on-premise only");
        return None;
    }
    tracing::info!(issuer = %server.issuer(), "oidc provider enabled");
    Some(auth_gateway::http::provider::routes_with_certificates(
        server,
        sessions,
        certificates,
    ))
}

/// The internal routes, for the host's update daemon: on their own port
/// (`ORQ_INTERNAL_ADDR`, which Compose publishes only on 127.0.0.1) and with
/// their token. Nothing without that variable.
pub(crate) async fn internal_routes(
    updates: application::UpdatesService,
    admin_access: Option<api::internal::AdminAccessRoute>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(addr) = env_value("ORQ_INTERNAL_ADDR") else {
        return Ok(());
    };
    let token_file = env_value("ORQ_INTERNAL_TOKEN_FILE")
        .ok_or("ORQ_INTERNAL_ADDR needs ORQ_INTERNAL_TOKEN_FILE")?;
    let token = api::internal::InternalToken::from_file(&token_file)?;
    let internal = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("ORQ_INTERNAL_ADDR {addr}: {e}"))?;
    tracing::info!(addr = %internal.local_addr()?, "internal routes");
    let router = api::internal::router(updates, admin_access, token);
    tokio::spawn(async move {
        if let Err(e) = axum::serve(internal, router).await {
            tracing::error!("internal routes: {e}");
        }
    });
    Ok(())
}
