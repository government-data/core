//! A tenant's warehouse on the main lake (0028's "a tenant's warehouse";
//! 0066 step 11, the user's ruling of 2026-11-13 on the warehouse): made,
//! when missing, by the same runs as the replication and just before it,
//! with the main lake's admin keys alone, so a warehouse is there for the
//! workers before a replica exists. Both are asked of the main lake's
//! adapter (decision 0078, step 10). Nothing in the application creates a
//! tenant (0065: the one tenant is made at installation), so these runs
//! are when a lake's keys are pasted and the periodic reconcile.

use auth_gateway::{Lake, ReplicationOutcome, ReplicationStep};
use domain::{AdminId, AuditEventType};

use super::lake_replication::Lakes;
use super::{LakeAdminError, MachinesService, Origin};
use crate::lake::{
    LakeAdmin, LakeError, ReplicaTarget, ReplicationState, RetentionRules, WarehouseMade,
};

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
        main: &dyn LakeAdmin,
        warehouse: &str,
        lakes: Option<&Lakes>,
        by: Option<(&AdminId, &Origin)>,
    ) -> Result<ReplicationOutcome, LakeAdminError> {
        let backend = |e: domain::RepositoryError| LakeAdminError::Backend(e.to_string());
        let made = main
            .ensure_warehouse(warehouse, &RetentionRules::default())
            .await;
        match made {
            Ok(WarehouseMade::Made) => {
                let event = audit_made(warehouse, by, self.clock.now());
                self.record(event).await.map_err(backend)?;
            }
            Ok(WarehouseMade::Present) => {}
            Ok(WarehouseMade::Failed { step, detail }) => {
                let step = step_named(&step)?;
                return Ok(ReplicationOutcome::Failed { step, detail });
            }
            Err(e) => {
                let step = ReplicationStep::WarehouseBucket;
                let detail = e.to_string();
                return Ok(ReplicationOutcome::Failed { step, detail });
            }
        }
        let Some(lakes) = lakes else {
            return Ok(ReplicationOutcome::Made);
        };
        let target = ReplicaTarget {
            endpoint: lakes.replica.clone(),
            source_pool: lakes.source_pool,
        };
        replication_outcome(main.replicate(warehouse, &target).await)
    }
}

/// What the lake said of a warehouse's replication, as the state keeps it:
/// `404 unknown` is a warehouse the source does not have; a lake not
/// reached fails the first step.
fn replication_outcome(
    state: Result<ReplicationState, LakeError>,
) -> Result<ReplicationOutcome, LakeAdminError> {
    match state {
        Ok(ReplicationState::Configured) => Ok(ReplicationOutcome::Configured),
        Ok(ReplicationState::Failed { step, detail }) => Ok(ReplicationOutcome::Failed {
            step: step_named(&step)?,
            detail,
        }),
        Ok(ReplicationState::NotConfigured) => Err(LakeAdminError::Backend(
            "the lake answered a replication not configured".into(),
        )),
        Err(LakeError::Unknown(_)) => Ok(ReplicationOutcome::NoWarehouse),
        Err(e) => Ok(ReplicationOutcome::Failed {
            step: ReplicationStep::SourceBucket,
            detail: e.to_string(),
        }),
    }
}

/// A step the lake named, in the application's list (decision 0078, the
/// user's ruling of 2026-11-24); any other is the lake's error.
fn step_named(step: &str) -> Result<ReplicationStep, LakeAdminError> {
    ReplicationStep::from_name(step)
        .ok_or_else(|| LakeAdminError::Backend(format!("the lake named no known step: {step}")))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_lake_said_of_a_replication_is_kept_as_the_state_keeps_it() {
        let failed = ReplicationState::Failed {
            step: "remote_target".into(),
            detail: "HTTP 502".into(),
        };
        assert_eq!(
            replication_outcome(Ok(failed)),
            Ok(ReplicationOutcome::Failed {
                step: ReplicationStep::RemoteTarget,
                detail: "HTTP 502".into(),
            })
        );
        let unknown = Err(LakeError::Unknown("PUT w".into()));
        assert_eq!(
            replication_outcome(unknown),
            Ok(ReplicationOutcome::NoWarehouse)
        );
        let down = replication_outcome(Err(LakeError::Unavailable("PUT w".into())));
        let first = ReplicationOutcome::Failed {
            step: ReplicationStep::SourceBucket,
            detail: "unavailable: PUT w".into(),
        };
        assert_eq!(down, Ok(first));
        let odd = ReplicationState::Failed {
            step: "dance".into(),
            detail: "x".into(),
        };
        assert!(replication_outcome(Ok(odd)).is_err());
    }
}
