//! The replication of the main lake into the replica (decision 0066 step
//! 11, the user's rulings of 2026-11-13): the warehouse the application
//! configures it for, and its periodic reconcile.
//!
//! The reconcile runs every `LAKE_REPLICATION_INTERVAL_SECS` (3600 by
//! default) and once soon after start. Once the main lake is known and its
//! keys pasted it makes the warehouse if it is missing there (the user's
//! ruling of 2026-11-13: the safety net of the warehouse made where the
//! installation is provisioned, which this application has no step for);
//! once the replica and its keys are too, it configures
//! their replication. Each run is idempotent, so every API replica may
//! run it.

use std::sync::Arc;
use std::time::Duration;

use application::{MachinesService, ReplicationReadiness};
use auth_gateway::{ReplicationOutcome, Warehouses};
use domain::RepositoryError;

/// The installation's one warehouse, as the quality module names it
/// (decision 0076, point 5).
pub(crate) struct TheWarehouse;

#[async_trait::async_trait]
impl Warehouses for TheWarehouse {
    async fn warehouses(&self) -> Result<Vec<String>, RepositoryError> {
        Ok(vec![::quality::lake::WAREHOUSE.to_string()])
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
            reconcile(&machines).await;
        }
    });
}

/// With the replication: its state and the warehouse; the lakes' adapters
/// are the machines' (`with_lake_admin`).
pub(crate) fn with_replication(
    machines: MachinesService,
    pool: &infrastructure::PgPool,
) -> MachinesService {
    machines.with_replication(
        Arc::new(infrastructure::PgReplicationState::new(pool.clone())),
        Arc::new(TheWarehouse),
    )
}
