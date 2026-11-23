//! What `serve` builds from the environment before mounting it: the OIDC
//! provider.

use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::SessionService;

use crate::config::env_value;

/// The OIDC provider, if there is a key (`OIDC_SIGNING_KEY`). The issuer is `OIDC_ISSUER` or, without it, the public address
/// with `/api` (Traefik strips it).
/// `certificates` says whether its token endpoint reads a machine's
/// certificate (`INTERNAL_CLIENT_CERT_HEADER`, decision 0066).
pub(crate) fn oidc_provider(
    server: Option<auth_gateway::AuthorizationServer>,
    sessions: SessionService,
    certificates: ClientCertificates,
) -> Option<axum::Router> {
    let server = server?;
    tracing::info!(issuer = %server.issuer(), "oidc provider enabled");
    Some(auth_gateway::http::provider::routes_with_certificates(
        server,
        sessions,
        certificates,
    ))
}

/// The internal routes, for the host's update daemon: on their own port
/// (`QD_INTERNAL_ADDR`, which Compose publishes only on 127.0.0.1) and with
/// their token. Nothing without that variable.
pub(crate) async fn internal_routes(
    updates: application::UpdatesService,
    admin_access: Option<api::internal::AdminAccessRoute>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(addr) = env_value("QD_INTERNAL_ADDR") else {
        return Ok(());
    };
    let token_file = env_value("QD_INTERNAL_TOKEN_FILE")
        .ok_or("QD_INTERNAL_ADDR needs QD_INTERNAL_TOKEN_FILE")?;
    let token = api::internal::InternalToken::from_file(&token_file)?;
    let internal = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("QD_INTERNAL_ADDR {addr}: {e}"))?;
    tracing::info!(addr = %internal.local_addr()?, "internal routes");
    let router = api::internal::router(updates, admin_access, token);
    tokio::spawn(async move {
        if let Err(e) = axum::serve(internal, router).await {
            tracing::error!("internal routes: {e}");
        }
    });
    Ok(())
}
