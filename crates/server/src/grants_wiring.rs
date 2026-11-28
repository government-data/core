//! The job grants route (decision 0090, point 7.2; rulings 84 and 97):
//! `GET /internal/quality/v1/job-grants`, which the Trino rules merge asks
//! with a machine token of the core's audience, filled from the catalog's
//! `main`, with the people's grants (decision 0089) and each open console
//! run's. Not served without the OIDC provider or the catalog.

use std::sync::Arc;

use application::PlatformService;
use auth_gateway::AuthorizationServer;
use axum::Router;
use infrastructure::PgPool;
use quality::catalog::Catalog;

/// The machine tokens the route takes: the core's audience (ruling 97).
struct CoreTokens(AuthorizationServer);

impl quality::worker_api::MachineTokens for CoreTokens {
    fn client(&self, token: &str) -> Option<quality::worker_api::MachineCaller> {
        let (client, machine) = self
            .0
            .machine_caller(token, quality::job_grants_api::AUDIENCE)?;
        Some(quality::worker_api::MachineCaller { client, machine })
    }
}

/// The route, or nothing without a provider or a catalog.
pub(crate) fn routes(
    provider: Option<AuthorizationServer>,
    pool: &PgPool,
    admins: Arc<dyn auth_gateway::AdminRepository>,
    platform: PlatformService,
) -> Result<Router, Box<dyn std::error::Error>> {
    let (Some(p), Some(dolt)) = (provider, crate::catalog_wiring::DoltSnapshots::from_env()?)
    else {
        return Ok(Router::new());
    };
    let catalog: Arc<dyn Catalog> = dolt;
    let people = Arc::new(application::platform::PlatformGrantees::new(
        admins, platform,
    ));
    Ok(quality::job_grants_api::routes_with_consoles(
        catalog,
        Arc::new(CoreTokens(p)),
        Some(people),
        pool.clone(),
    ))
}
