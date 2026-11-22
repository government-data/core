//! The API as Trino's client, for its coordinator's hold only (0066,
//! point 9): the API asks it what it runs, with a fresh token of its own
//! machine client, signed here by the OIDC provider. No job reads through
//! Trino any more (decision 0077); the hold goes in that decision's step 9.

use std::time::Duration;

use application::{TrinoJobs, TrinoQueries};
use async_trait::async_trait;
use auth_gateway::AuthorizationServer;
use infrastructure::PgPool;
use quality::tenants::Tenants;
use quality::trino_hold;
use trino_client::TrinoClient;

use crate::tenants::CellTenants;

/// The audience of the API's token for Trino (decision 0030).
const AUDIENCE: &str = "trino";

pub(crate) struct ApiTrino {
    client: TrinoClient,
    provider: AuthorizationServer,
    client_id: String,
}

impl ApiTrino {
    /// From `QUALITY_TRINO_URL` (the coordinator's protocol,
    /// `https://…:8490`), trusting `QUALITY_TRINO_CA_FILE` too if set, as the
    /// machine client `QUALITY_TRINO_CLIENT` (`qd-api` by default). `None`
    /// without a URL; an error with one but no OIDC provider to sign its
    /// tokens.
    pub(crate) fn from_env(provider: Option<AuthorizationServer>) -> Result<Option<Self>, String> {
        let var = |n: &str| std::env::var(n).ok().filter(|v| !v.trim().is_empty());
        let Some(url) = var("QUALITY_TRINO_URL") else {
            return Ok(None);
        };
        let provider =
            provider.ok_or("QUALITY_TRINO_URL needs the OIDC provider (OIDC_SIGNING_KEY)")?;
        let ca = var("QUALITY_TRINO_CA_FILE")
            .map(|f| std::fs::read(&f).map_err(|e| format!("QUALITY_TRINO_CA_FILE {f}: {e}")))
            .transpose()?;
        let client = TrinoClient::new(&url, ca.as_deref())
            .map_err(|e| format!("QUALITY_TRINO_URL {url}: {e}"))?;
        Ok(Some(Self {
            client,
            provider,
            client_id: client_id(),
        }))
    }
}

/// The API's own machine client in Trino, `QUALITY_TRINO_CLIENT`.
fn client_id() -> String {
    std::env::var("QUALITY_TRINO_CLIENT")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "qd-api".into())
}

/// Trino's own answer to whether it runs a query (0066, point 9), asked
/// now as the API's client.
#[async_trait]
impl TrinoQueries for ApiTrino {
    async fn active(&self) -> Result<u64, String> {
        let token = self
            .provider
            .own_machine_token(&self.client_id, AUDIENCE)
            .await
            .map_err(|e| format!("token of {}: {}", self.client_id, e.code()))?;
        self.client
            .active_queries(&token.access_token)
            .await
            .map_err(|e| e.to_string())
    }
}

/// The jobs that use Trino, held and released in every tenant of the
/// cell, as the rules' route reads them (0066, point 9).
pub(crate) struct QueueTrinoJobs {
    pub(crate) pool: PgPool,
    pub(crate) tenants: CellTenants,
}

#[async_trait]
impl TrinoJobs for QueueTrinoJobs {
    async fn hold(&self, machine: &str, ttl: Duration) -> Result<u64, String> {
        let mut running = 0;
        for tenant in self.tenants.tenants().await? {
            let held = tenancy::within(tenant, trino_hold::hold(&self.pool, machine, ttl));
            running += held.await.map_err(|e| e.to_string())?;
        }
        Ok(running)
    }

    /// Every tenant's, even when one fails: the rest are not left held.
    async fn release(&self) -> Result<bool, String> {
        let (mut in_force, mut failed) = (false, None);
        for tenant in self.tenants.tenants().await? {
            match tenancy::within(tenant, trino_hold::release(&self.pool)).await {
                Ok(was) => in_force |= was,
                Err(e) => failed = Some(e.to_string()),
            }
        }
        failed.map_or(Ok(in_force), Err)
    }
}
