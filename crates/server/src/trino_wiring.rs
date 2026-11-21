//! The API as Trino's client: no source keeps a catalog there any more
//! (decision 0077); the API asks it what it runs, with a fresh token of
//! its own machine client, signed here by the OIDC provider. A job that
//! reads a source on Trino gets a token of its own, signed the same way
//! (decision 0062), and Trino reads its access-control rules from the API, which
//! knows the coordinator by its certificate (0066, point 9), or in
//! development by a secret in the query.

use std::sync::Arc;
use std::time::Duration;

use application::{TrinoJobs, TrinoQueries};
use async_trait::async_trait;
use auth_gateway::AuthorizationServer;
use infrastructure::PgPool;
use quality::job_trino::{TrinoAccess, TrinoJobTokens};
use quality::machine_certs::MachineCertificates;
use quality::trino_hold;
use quality::trino_rules_api::{RulesAccess, RulesKey, Tenants};
use trino_client::TrinoClient;

/// The audience of Trino's tokens (decision 0030).
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

/// The tenants of this API's cell, as the scheduler finds them: the main
/// one on-premise, the directory's active ones online.
#[derive(Clone)]
pub(crate) struct CellTenants {
    directory: Option<Arc<dyn tenancy::Directory>>,
    cell: String,
}

impl From<&crate::tenants::Tenancy> for CellTenants {
    fn from(tenancy: &crate::tenants::Tenancy) -> Self {
        Self {
            directory: tenancy.directory.clone(),
            cell: tenancy.cell.clone(),
        }
    }
}

#[async_trait]
impl Tenants for CellTenants {
    async fn tenants(&self) -> Result<Vec<tenancy::Tenant>, String> {
        let Some(directory) = &self.directory else {
            return Ok(vec![tenancy::Tenant::main()]);
        };
        let records = directory.in_cell(&self.cell).await?;
        Ok(records
            .into_iter()
            .filter(|r| r.status == tenancy::Status::Active)
            .map(|r| r.tenant)
            .collect())
    }
}

/// What the route Trino reads its rules from needs: Trino's coordinator
/// known by its certificate (`machines`, where the certificate header is
/// trusted: decision 0066, point 9), and in development, until its stack
/// has certificates, the secret Trino sends in the query (0062, point 6),
/// from `QUALITY_TRINO_RULES_KEY` or its `_FILE`, which production does not
/// set. `None` with neither (the route is not served); an error for a
/// secret a query does not carry as it is, or a short one.
pub(crate) fn rules_from_env(
    tenants: CellTenants,
    machines: Option<Arc<dyn MachineCertificates>>,
) -> Result<Option<RulesAccess>, String> {
    let key = infrastructure::oidc_es256::key_from_env("QUALITY_TRINO_RULES_KEY")?
        .map(|key| RulesKey::parse(key.trim()))
        .transpose()
        .map_err(|e| format!("QUALITY_TRINO_RULES_KEY: {e}"))?;
    if key.is_none() && machines.is_none() {
        return Ok(None);
    }
    if key.is_some() {
        tracing::warn!("Trino's rules are served with QUALITY_TRINO_RULES_KEY: development only");
    }
    Ok(Some(RulesAccess {
        key,
        machines,
        client: client_id(),
        tenants: Arc::new(tenants),
    }))
}

/// Signs a job's token as the API's own machine client.
pub(crate) struct JobTokens {
    provider: AuthorizationServer,
    client_id: String,
}

impl ApiTrino {
    /// Its jobs' tokens, signed as its own client.
    pub(crate) fn job_tokens(&self) -> JobTokens {
        JobTokens {
            provider: self.provider.clone(),
            client_id: self.client_id.clone(),
        }
    }
}

#[async_trait]
impl TrinoJobTokens for JobTokens {
    async fn issue(&self, subject: &str, actor: &str) -> Result<TrinoAccess, String> {
        let token = self
            .provider
            .job_token(&self.client_id, AUDIENCE, subject, actor)
            .await
            .map_err(|e| format!("job token as {}: {}", self.client_id, e.code()))?;
        Ok(TrinoAccess {
            token: token.access_token,
            expires_in: token.expires_in,
        })
    }
}

/// Trino's own answer to whether it runs a query (0066, point 9), asked
/// now as the API's client, whom the rules let view every query.
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
