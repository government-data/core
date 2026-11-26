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
use infrastructure::lake_admins::AdapterLakeAdmins;
use infrastructure::mimir_series::MimirSeries;
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
    // Each lake's adapter (decision 0078): the main lake's at
    // `LAKE_ADMIN_URL`, a replica's on its machine; the client sends the
    // kept keys with each call, presenting the application VM's
    // certificate (`LAKE_ADMIN_CERT_FILE` or `LAKE_CLIENT_CERT_FILE`) and
    // trusting the installation's CA (`LAKE_ADMIN_CA_FILE` or `CA_CERT_FILE`).
    let keys = Arc::new(PgLakeAdminKeys::new(pool.clone(), sealer));
    let machines = match AdapterLakeAdmins::from_env(keys) {
        Ok(api) => {
            tokio::spawn(about_the_main_lake(api.clone()));
            let machines = machines
                .with_lake_admin(Arc::new(api))
                // The main lake, where the application reaches it (0066 step 11).
                .with_main_lake(std::env::var("LAKE_ENDPOINT").ok());
            let machines = replication_wiring::with_replication(machines, pool);
            replication_wiring::spawn_reconcile(machines.clone());
            tokio::spawn(sts_issuer_user(machines.clone()));
            machines
        }
        Err(e) => {
            tracing::warn!(error = %e, "the lakes' adapters are not reachable from here");
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

/// What the main lake's adapter says it is, logged once at start (decision
/// 0078, step 13): whether it serves `lake-admin/1`, or why it did not
/// answer. Settings → Lake asks its health.
async fn about_the_main_lake(admins: AdapterLakeAdmins) {
    use contracts::lake_admin::PROTOCOL;
    match admins.main_about().await {
        Ok(about) if about.protocols.iter().any(|p| p == PROTOCOL) => tracing::info!(
            implementation = %about.implementation,
            version = %about.version,
            "the main lake's adapter serves {PROTOCOL}"
        ),
        Ok(about) => tracing::warn!(
            protocols = ?about.protocols,
            "the main lake's adapter does not serve {PROTOCOL}"
        ),
        Err(e) => tracing::warn!(error = %e, "the main lake's adapter did not answer"),
    }
}

/// The STS issuer's lake user, made with the keys the package gives
/// (`RUSTFS_STS_ISSUER_*`) once the API starts (decision 0091, ruling 51);
/// asking again changes nothing. No key is logged.
async fn sts_issuer_user(machines: MachinesService) {
    use application::lake::{LakeCredentialKind, LakeKeys};
    let (access_key, secret_key) = match infrastructure::rustfs_sts::issuer_keys_from_env() {
        Ok(Some(keys)) => keys,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(error = %e, "the STS issuer's keys cannot be read");
            return;
        }
    };
    let keys = LakeKeys {
        access_key,
        secret_key,
    };
    // The lake may start after the API: tried again each minute for an hour
    // while the lake may answer later.
    for _ in 0..60 {
        match machines
            .ensure_lake_user(LakeCredentialKind::StsIssuer, &keys)
            .await
        {
            Ok(()) => return tracing::info!("the STS issuer's lake user is in place"),
            Err(e) if e.retryable() => {
                tracing::warn!(error = %e, "the STS issuer's lake user: the lake did not answer");
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
            Err(e) => {
                return tracing::warn!(error = %e, "the STS issuer's lake user could not be made");
            }
        }
    }
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
