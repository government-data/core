//! A tenant's warehouse on the main lake (0028's "a tenant's warehouse";
//! 0066 step 11, the user's ruling of 2026-11-13 on the warehouse): made,
//! when missing, by the same runs as the replication and just before it,
//! with the main lake's admin keys alone, so a warehouse is there for the
//! workers before a replica exists. Nothing in the application creates a
//! tenant (0065: the one tenant is made at installation), so these runs
//! are when a lake's keys are pasted and the periodic reconcile.

use auth_gateway::{
    Lake, ReplicationOutcome, ReplicationStep, WarehouseOutcome, WarehouseReplication,
};
use domain::{AdminId, AuditEventType, RepositoryError};

use super::lake_replication::{Lakes, Replication};
use super::{LakeAdminError, MachinesService, Origin};

impl MachinesService {
    /// The main lake's base URL, when it is known and its admin keys are
    /// kept: what making a warehouse needs. A read.
    pub(super) async fn warehouse_source(&self) -> Result<Option<String>, LakeAdminError> {
        let Some(source) = self.main_lake.clone() else {
            return Ok(None);
        };
        let store = self
            .lake_admin
            .as_ref()
            .ok_or(LakeAdminError::Unavailable)?;
        let kept = store.kept(&Lake::main()).await;
        let kept = kept.map_err(|e| LakeAdminError::Backend(e.to_string()))?;
        Ok(kept.map(|_| source))
    }

    /// One warehouse: made on the main lake when missing (audited when it
    /// is), then its replication when `lakes` are ready; `made` otherwise.
    pub(super) async fn warehouse_outcome(
        &self,
        replication: &Replication,
        source: &str,
        warehouse: &str,
        lakes: Option<&Lakes>,
        by: Option<(&AdminId, &Origin)>,
    ) -> Result<ReplicationOutcome, LakeAdminError> {
        let backend = |e: RepositoryError| LakeAdminError::Backend(e.to_string());
        let made = replication.maker.ensure(source, warehouse).await;
        match made.map_err(backend)? {
            WarehouseOutcome::NoKeys => {
                return Ok(ReplicationOutcome::NoKeys { lake: Lake::main() })
            }
            WarehouseOutcome::Failed { step, detail } => {
                return Ok(ReplicationOutcome::Failed { step, detail })
            }
            WarehouseOutcome::Made => {
                let event = audit_made(warehouse, by, self.clock.now());
                self.record(event).await.map_err(backend)?;
            }
            WarehouseOutcome::Present => {}
        }
        let Some(lakes) = lakes else {
            return Ok(ReplicationOutcome::Made);
        };
        let plan = WarehouseReplication {
            warehouse: warehouse.to_string(),
            source: lakes.source.clone(),
            replica: lakes.replica.clone(),
            proxy: lakes.proxy.clone(),
        };
        replication.api.configure(&plan).await.map_err(backend)
    }
}

/// Whether a step is making the warehouse, not its replication.
pub(super) fn makes_the_warehouse(step: ReplicationStep) -> bool {
    matches!(
        step,
        ReplicationStep::WarehouseBucket | ReplicationStep::WarehouseTables
    )
}

/// Where the warehouse is in the audit: on the main lake.
pub(super) fn on_the_main_lake(warehouse: &str) -> String {
    format!("lake/{}/{warehouse}", Lake::main().key())
}

/// `lake:warehouse:made` on `lake/pool:main/<warehouse>`, by the person
/// or the application.
fn audit_made(
    warehouse: &str,
    by: Option<(&AdminId, &Origin)>,
    at: std::time::SystemTime,
) -> domain::NewActionAudit {
    domain::NewActionAudit {
        event_type: AuditEventType::ResourceChanged,
        admin_id: by.map(|(a, _)| a.clone()),
        service_account: by.is_none().then(|| super::RECONCILER.to_string()),
        job_id: None,
        action: "lake:warehouse:made".into(),
        resource: on_the_main_lake(warehouse),
        ip: by.map_or_else(|| "-".to_string(), |(_, o)| o.ip.clone()),
        user_agent: by.and_then(|(_, o)| o.user_agent.clone()),
        occurred_at: at,
    }
}
