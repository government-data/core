//! The replication of the main lake into the replica (decision 0066 step
//! 11, the user's rulings of 2026-11-13): the tenants' warehouses the
//! application configures it for, and its periodic reconcile.
//!
//! The reconcile runs every `LAKE_REPLICATION_INTERVAL_SECS` (3600 by
//! default) and once soon after start, under the main tenant, whose rows
//! keep the installation's lake keys and state. Once the main lake is
//! known and its keys pasted it makes every tenant's warehouse that is
//! missing there (the user's ruling of 2026-11-13: the safety net of the
//! warehouse made where the tenant is provisioned, which this application
//! has no step for); once the replica and its keys are too, it configures
//! their replication. Each run is idempotent, so every API replica may
//! run it.

use std::sync::Arc;
use std::time::Duration;

use application::{MachinesService, ReplicationReadiness};
use auth_gateway::{ReplicationOutcome, Warehouses};
use domain::RepositoryError;

use crate::trino_wiring::CellTenants;
use ::quality::trino_rules_api::Tenants;

/// The cell's tenants' warehouses: `qd-tenant-<id>`, as the quality module
/// names them.
pub(crate) struct TenantWarehouses(pub(crate) CellTenants);

#[async_trait::async_trait]
impl Warehouses for TenantWarehouses {
    async fn warehouses(&self) -> Result<Vec<String>, RepositoryError> {
        let tenants = self.0.tenants().await.map_err(RepositoryError::Backend)?;
        Ok(tenants
            .iter()
            .filter_map(|t| uuid::Uuid::parse_str(&t.id).ok())
            .map(::quality::lake::warehouse)
            .collect())
    }
}

fn interval() -> Duration {
    let secs = std::env::var("LAKE_REPLICATION_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(3600);
    Duration::from_secs(secs)
}

/// One run of the reconcile, its outcome logged without a key or a
/// detail: how many warehouses are made, configured or failed.
pub(crate) async fn reconcile(machines: &MachinesService) {
    let status = match machines.replicate(None).await {
        Ok(status) => status,
        Err(e) => return tracing::warn!(error = ?e, "lake replication"),
    };
    let count = |f: fn(&ReplicationOutcome) -> bool| {
        status.warehouses.iter().filter(|w| f(&w.outcome)).count()
    };
    let made = count(|o| *o == ReplicationOutcome::Made);
    let configured = count(|o| *o == ReplicationOutcome::Configured);
    let failed = count(|o| matches!(o, ReplicationOutcome::Failed { .. }));
    let ready = matches!(status.readiness, ReplicationReadiness::Ready(_));
    if failed > 0 {
        tracing::warn!(
            made,
            configured,
            failed,
            "lake replication: some warehouses failed"
        );
    } else if ready {
        tracing::info!(configured, "lake replication reconciled");
    } else {
        tracing::debug!(readiness = ?status.readiness, made, "lake replication: not ready");
    }
}

/// The periodic reconcile, under the main tenant.
pub(crate) fn spawn_reconcile(machines: MachinesService) {
    let every = interval();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            tenancy::within(tenancy::Tenant::main(), reconcile(&machines)).await;
        }
    });
}

/// With the replication: the lake's adapter (`api`), which also makes a
/// missing warehouse on the main lake, its state and the cell's
/// warehouses.
pub(crate) fn with_replication(
    machines: MachinesService,
    api: Arc<infrastructure::rustfs_admin::RustfsLakeAdmin>,
    pool: &infrastructure::PgPool,
    tenants: CellTenants,
) -> MachinesService {
    machines.with_replication(
        api.clone(),
        api,
        Arc::new(infrastructure::PgReplicationState::new(pool.clone())),
        Arc::new(TenantWarehouses(tenants)),
    )
}
