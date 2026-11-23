//! The installation's machines (decisions 0029 and 0066): their use cases
//! over PostgreSQL, the administrators' routes, the route a machine joins
//! with, the one it renews its certificate with, Dolt's hold before it
//! restarts, the lake's gate, and Grafana's alerts
//! about the machines (0072); and what a new machine is installed with
//! (`/install/…`: `join.sh`, the CA, and its role's compose file, its
//! entrypoints and a lake's root keys for a usable join token), the join
//! command built for `APP_PUBLIC_URL`. The CA is read once, here, at start
//! (`CA_CERT_FILE`, `CA_KEY_FILE`): without one the server starts, no join
//! token is issued and no machine joins or renews; with only half of one
//! it does not start.
//!
//! `INTERNAL_CLIENT_CERT_HEADER=trusted` lets the renew route, the OIDC
//! provider's token endpoint, Dolt's hold, the lake's gate and the
//! alerts route read the client certificate Traefik's internal entry point
//! passes. Set it only where the API cannot be reached but through Traefik:
//! anyone who reaches the API directly could write that header. Unset, the
//! route answers 503.

use std::sync::Arc;

use api::rate_limit::RateLimitConfig;
use application::MachinesService;
use auth_gateway::http::client_cert::ClientCertificates;
use auth_gateway::SessionService;
use infrastructure::ca::{Ca, X509CertificateReader};
use infrastructure::mimir_series::MimirSeries;
use infrastructure::rustfs_admin::RustfsLakeAdmin;
use infrastructure::{
    EmbeddedInstallFiles, PgAuditRepository, PgLakeAdminKeys, PgLakePools, PgLakeRootKeys,
    PgMachineAlerts, PgMachineSessions, PgMachines, PgPool, RingJoinTokenMint, SecretBox,
    SystemClock,
};

use crate::config::{env_value, public_url_from_env};
use crate::replication_wiring;

/// The machines' routes, both for people (a session and
/// `admin:ManageMachines`) and for machines: the install routes that check
/// a join token, limited by `install_rate_limit` per client address
/// (`INSTALL_RATE_LIMIT_*`), joining with a token, by `join_rate_limit`
/// (the login's), and renewing with a certificate.
pub(crate) fn routes(
    machines: MachinesService,
    sessions: SessionService,
    join_rate_limit: RateLimitConfig,
    install_rate_limit: RateLimitConfig,
) -> Result<axum::Router, Box<dyn std::error::Error>> {
    let certificates = client_certificates_from_env()?;
    Ok(api::machines::routes(machines.clone(), sessions)
        .merge(api::machines::install::routes(
            machines.clone(),
            install_rate_limit,
        ))
        .merge(api::machines::join::routes(
            machines.clone(),
            join_rate_limit,
        ))
        .merge(api::machines::renew::routes(
            machines.clone(),
            certificates.clone(),
        ))
        .merge(api::machines::dolt::routes(
            machines.clone(),
            certificates.clone(),
        ))
        .merge(api::machines::alerts::routes(
            machines.clone(),
            certificates.clone(),
            crate::metrics_wiring::unknown_alerts(),
        ))
        .merge(api::machines::lake::routes(machines, certificates)))
}

/// The machines' use cases over PostgreSQL, with the installation's CA if
/// it has one, the replication of the warehouse, and the
/// lakes' root keys and admin keys sealed with the settings' key (`sealer`,
/// 0066 step 11).
pub(crate) fn service(
    pool: &PgPool,
    sealer: SecretBox,
) -> Result<MachinesService, Box<dyn std::error::Error>> {
    let machines = base(pool);
    let ca = Ca::from_env()?;
    let files = EmbeddedInstallFiles::new(ca.as_ref().map(Ca::cert_pem))?;
    let tokens = Arc::new(PgMachines::new(pool.clone()));
    let machines = machines
        .with_install(Arc::new(files), tokens, &public_url_from_env())
        // Where a joined machine's Alloy pushes its metrics (0071, point 14).
        .with_metrics_push_url(std::env::var("MACHINES_METRICS_PUSH_URL").ok().as_deref())
        .with_lake_keys(Arc::new(PgLakeRootKeys::new(pool.clone(), sealer.clone())))
        .with_lake_admin_keys(Arc::new(PgLakeAdminKeys::new(pool.clone(), sealer.clone())))
        .with_lake_pools(Arc::new(PgLakePools::new(pool.clone())))
        // Grafana's alerts about the machines (0072, point 11).
        .with_machine_alerts(Arc::new(PgMachineAlerts::new(pool.clone())));
    let machines = with_series(machines);
    // The lakes' admin API reads the kept keys itself, and reaches a node's
    // gate with the application VM's certificate, as the STS issuer does.
    let keys = Arc::new(PgLakeAdminKeys::new(pool.clone(), sealer));
    let machines = match RustfsLakeAdmin::from_env(keys) {
        Ok(api) => {
            let api = Arc::new(api);
            let machines = machines
                .with_lake_admin(api.clone())
                // The main lake, where the application reaches it (0066 step 11).
                .with_main_lake(std::env::var("LAKE_ENDPOINT").ok());
            let machines = replication_wiring::with_replication(machines, api, pool);
            replication_wiring::spawn_reconcile(machines.clone());
            machines
        }
        Err(e) => {
            tracing::warn!(error = %e, "the lakes' admin API is not reachable from here");
            machines
        }
    };
    Ok(match ca {
        Some(ca) => {
            tracing::info!("machines can join: the installation's CA is loaded");
            machines.with_signer(Arc::new(ca))
        }
        None => {
            tracing::warn!("no CA (CA_CERT_FILE, CA_KEY_FILE): machines cannot join");
            machines
        }
    })
}

/// The machines' charts read from Mimir (`MIMIR_QUERY_URL`, 0072 point 13),
/// presenting the application VM's certificate as the lake's client does.
/// Unset or unusable, the charts say "metrics not available".
fn with_series(machines: MachinesService) -> MachinesService {
    match MimirSeries::from_env() {
        Ok(Some(series)) => machines.with_machine_series(Arc::new(series)),
        Ok(None) => {
            tracing::info!("no MIMIR_QUERY_URL: the machines' charts are not available");
            machines
        }
        Err(e) => {
            tracing::warn!(error = %e, "the machines' charts cannot read Mimir");
            machines
        }
    }
}

fn base(pool: &PgPool) -> MachinesService {
    MachinesService::new(
        Arc::new(PgMachines::new(pool.clone())),
        Arc::new(RingJoinTokenMint::new()),
        Arc::new(PgAuditRepository::new(pool.clone())),
        Arc::new(SystemClock),
    )
    .with_sessions(Arc::new(PgMachineSessions::new(pool.clone())))
}

/// Whether the client certificate header is read, by
/// `INTERNAL_CLIENT_CERT_HEADER`: for renewing and for the machines' tokens.
pub(crate) fn client_certificates_from_env() -> Result<ClientCertificates, String> {
    client_certificates(env_value("INTERNAL_CLIENT_CERT_HEADER").as_deref())
}

/// `trusted`, or unset; anything else stops the start, rather than leave a
/// typo deciding whether a header is believed.
fn client_certificates(setting: Option<&str>) -> Result<ClientCertificates, String> {
    match setting {
        None => Ok(ClientCertificates::not_trusted()),
        Some("trusted") => {
            tracing::info!("the client certificate header of the internal entry point is trusted");
            Ok(ClientCertificates::trusting(Arc::new(
                X509CertificateReader,
            )))
        }
        Some(other) => Err(format!(
            "INTERNAL_CLIENT_CERT_HEADER is {other:?}: `trusted` or unset"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_client_certificate_header_is_trusted_only_when_said() {
        assert!(!client_certificates(None).unwrap().is_trusted());
        assert!(client_certificates(Some("trusted")).unwrap().is_trusted());
        for other in ["true", "Trusted", "off", "yes"] {
            assert!(client_certificates(Some(other)).is_err(), "{other}");
        }
    }
}
