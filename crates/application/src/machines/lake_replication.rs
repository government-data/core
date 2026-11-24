//! The replication of the main lake into the replica (decisions 0027, 0028
//! and 0029, point 7; 0066 step 11, the user's rulings of 2026-11-13): for
//! every tenant's warehouse, configured by the main lake's adapter
//! (decision 0078) with both lakes' admin keys, which the core's client of
//! it alone reads. Run when a person pastes either
//! lake's keys and by the periodic reconcile; each run is idempotent, so a
//! second one changes nothing. A warehouse's change of state (configured,
//! or failed) is audited, by the person who pasted the keys or by the
//! application; never with a key. Before it, each warehouse is made on
//! the main lake when missing (`lake_warehouse`), which needs that lake's
//! keys alone.

use std::sync::Arc;

use auth_gateway::{
    Lake, ReplicationOutcome, ReplicationStateStore, WarehouseState, Warehouses, MAIN_POOL,
};
use domain::{AdminId, AuditEventType, RepositoryError};

use super::lake_replica::ReplicaState;
use super::lake_warehouse::{makes_the_warehouse, on_the_main_lake};
use super::{LakeAdminError, MachinesService, Origin};

/// Who the application is in the audit when it runs by itself.
pub const RECONCILER: &str = "qd-api";

/// The replication's ports, given together; the lakes' adapters are
/// [`MachinesService::with_lake_admin`]'s.
#[derive(Clone)]
pub(super) struct Replication {
    state: Arc<dyn ReplicationStateStore>,
    pub(super) warehouses: Arc<dyn Warehouses>,
}

/// Whether a run can configure anything, and with what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplicationReadiness {
    /// Both lakes known and both keys kept.
    Ready(Lakes),
    /// The application reaches no main lake (`LAKE_ENDPOINT`).
    NoAddress,
    /// No single replica: what Settings says of it.
    NoReplica(ReplicaState),
    /// `lake`'s admin keys are not kept.
    NoKeys { lake: Lake },
}

/// The two lakes, as a run reaches them: the main lake's address, the
/// replica's machine, and whether the main lake is a pool (its adapter
/// works out from it how the replication reaches the replica).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lakes {
    pub source: String,
    pub replica: String,
    pub source_pool: bool,
}

/// What Settings → Lake says of the replication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicationStatus {
    pub readiness: ReplicationReadiness,
    pub warehouses: Vec<WarehouseState>,
}

/// Whether a state is one a person should hear of: configured, or failed.
fn worth_auditing(before: Option<&ReplicationOutcome>, now: &ReplicationOutcome) -> bool {
    match now {
        ReplicationOutcome::Configured => before != Some(now),
        ReplicationOutcome::Failed { .. } => before != Some(now),
        _ => false,
    }
}

impl MachinesService {
    /// With the replication's state and the tenants' warehouses; the runs
    /// also need the lakes' adapters ([`Self::with_lake_admin`]).
    pub fn with_replication(
        mut self,
        state: Arc<dyn ReplicationStateStore>,
        warehouses: Arc<dyn Warehouses>,
    ) -> Self {
        self.replication = Some(Replication { state, warehouses });
        self
    }

    fn replication(&self) -> Result<&Replication, LakeAdminError> {
        self.replication.as_ref().ok_or(LakeAdminError::Unavailable)
    }

    /// Whether a run can configure anything now. A read.
    pub async fn replication_readiness(&self) -> Result<ReplicationReadiness, LakeAdminError> {
        let backend = |e: RepositoryError| LakeAdminError::Backend(e.to_string());
        let Some(source) = self.main_lake.clone() else {
            return Ok(ReplicationReadiness::NoAddress);
        };
        let source_pool = match &self.pools {
            Some(pools) => pools.find(MAIN_POOL).await.map_err(backend)?.is_some(),
            None => false,
        };
        let replica = match self.replica_machine().await? {
            Ok(name) => name,
            Err(state) => return Ok(ReplicationReadiness::NoReplica(state)),
        };
        let store = self
            .lake_admin
            .as_ref()
            .ok_or(LakeAdminError::Unavailable)?;
        for lake in [Lake::main(), Lake::Replica] {
            if store.kept(&lake).await.map_err(backend)?.is_none() {
                return Ok(ReplicationReadiness::NoKeys { lake });
            }
        }
        Ok(ReplicationReadiness::Ready(Lakes {
            source,
            replica,
            source_pool,
        }))
    }

    /// What the last runs left, and whether a run can go now. A read.
    pub async fn replication_status(&self) -> Result<ReplicationStatus, LakeAdminError> {
        let replication = self.replication()?;
        let readiness = self.replication_readiness().await?;
        let all = replication.state.all().await;
        let warehouses = all.map_err(|e| LakeAdminError::Backend(e.to_string()))?;
        Ok(ReplicationStatus {
            readiness,
            warehouses,
        })
    }

    /// Makes every tenant's warehouse on the main lake when missing, once
    /// that lake is known and its keys kept, and configures its
    /// replication if both lakes are ready; `by` is the person whose
    /// pasting started it, `None` for the reconcile.
    pub async fn replicate(
        &self,
        by: Option<(&AdminId, &Origin)>,
    ) -> Result<ReplicationStatus, LakeAdminError> {
        let replication = self.replication()?;
        let admins = self.lake_api.as_ref().ok_or(LakeAdminError::Unavailable)?;
        let backend = |e: RepositoryError| LakeAdminError::Backend(e.to_string());
        let Some(source) = self.warehouse_source().await? else {
            return self.replication_status().await;
        };
        let main = admins.for_lake(&Lake::main(), &source);
        let readiness = self.replication_readiness().await?;
        let lakes = match &readiness {
            ReplicationReadiness::Ready(lakes) => Some(lakes),
            _ => None,
        };
        let before = replication.state.all().await.map_err(backend)?;
        let mut names = replication.warehouses.warehouses().await.map_err(backend)?;
        names.sort();
        names.dedup();
        for warehouse in names {
            let outcome = self
                .warehouse_outcome(main.as_ref(), &warehouse, lakes, by)
                .await?;
            let now = self.clock.now();
            let was = before.iter().find(|s| s.warehouse == warehouse);
            replication
                .state
                .record(&warehouse, &outcome, now)
                .await
                .map_err(backend)?;
            if worth_auditing(was.map(|s| &s.outcome), &outcome) {
                let event = audit_replication(&warehouse, &outcome, by, now);
                self.record(event).await.map_err(backend)?;
            }
        }
        self.replication_status().await
    }

    /// The replica's machine by name, or what Settings says instead.
    pub(super) async fn replica_machine(
        &self,
    ) -> Result<Result<String, ReplicaState>, LakeAdminError> {
        use auth_gateway::{MachineRole, MachineState};
        let backend = |e: RepositoryError| LakeAdminError::Backend(e.to_string());
        let machines = self.registry.list().await.map_err(backend)?;
        let replicas = machines
            .iter()
            .filter(|m| m.role == MachineRole::LakeReplica);
        let (serving, pending): (Vec<_>, Vec<_>) = replicas
            .filter(|m| m.state != MachineState::Retired)
            .partition(|m| matches!(m.state, MachineState::Approved | MachineState::Draining));
        Ok(match serving.as_slice() {
            [] => {
                let pending = pending.first().map(|m| m.name.clone());
                Err(ReplicaState::NoMachine { pending })
            }
            [one] => Ok(one.name.clone()),
            several => {
                let names = several.iter().map(|m| m.name.clone()).collect();
                Err(ReplicaState::SeveralMachines { names })
            }
        })
    }
}

/// `lake:replication:configured` or `…:failed:<step>` on
/// `lake/replica/<warehouse>`, by the person or the application; a step of
/// making the warehouse, `lake:warehouse:failed:<step>` on
/// `lake/pool:main/<warehouse>`.
fn audit_replication(
    warehouse: &str,
    outcome: &ReplicationOutcome,
    by: Option<(&AdminId, &Origin)>,
    at: std::time::SystemTime,
) -> domain::NewActionAudit {
    let mut resource = format!("lake/replica/{warehouse}");
    let action = match outcome {
        ReplicationOutcome::Failed { step, .. } if makes_the_warehouse(*step) => {
            resource = on_the_main_lake(warehouse);
            format!("lake:warehouse:failed:{}", step.as_str())
        }
        ReplicationOutcome::Failed { step, .. } => {
            format!("lake:replication:failed:{}", step.as_str())
        }
        _ => "lake:replication:configured".into(),
    };
    domain::NewActionAudit {
        event_type: AuditEventType::ResourceChanged,
        admin_id: by.map(|(a, _)| a.clone()),
        service_account: by.is_none().then(|| RECONCILER.to_string()),
        job_id: None,
        action,
        resource,
        ip: by.map_or_else(|| "-".to_string(), |(_, o)| o.ip.clone()),
        user_agent: by.and_then(|(_, o)| o.user_agent.clone()),
        occurred_at: at,
    }
}
