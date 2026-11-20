//! The replication of the main lake into the replica (decisions 0027, 0028
//! and 0029, point 7; 0066 step 11, the user's rulings of 2026-11-13): for
//! every tenant's warehouse, configured by the lake's adapter with both
//! lakes' admin keys, which it alone reads. Run when a person pastes either
//! lake's keys and by the periodic reconcile; each run is idempotent, so a
//! second one changes nothing. A warehouse's change of state (configured,
//! or failed) is audited, by the person who pasted the keys or by the
//! application; never with a key. Before it, each warehouse is made on
//! the main lake when missing (`lake_warehouse`), which needs that lake's
//! keys alone.

use std::sync::Arc;

use auth_gateway::{
    Lake, LakeReplication, LakeWarehouses, ReplicationOutcome, ReplicationStateStore,
    WarehouseState, Warehouses, MAIN_POOL, REPLICATION_ALIAS, REPLICATION_PROXY_PORT,
};
use domain::{AdminId, AuditEventType, RepositoryError};

use super::lake_replica::ReplicaState;
use super::lake_warehouse::{makes_the_warehouse, on_the_main_lake};
use super::{LakeAdminError, MachinesService, Origin};

/// Who the application is in the audit when it runs by itself.
pub const RECONCILER: &str = "qd-api";

/// The replication's ports, given together.
#[derive(Clone)]
pub(super) struct Replication {
    pub(super) api: Arc<dyn LakeReplication>,
    /// Makes a warehouse on the main lake when it is missing.
    pub(super) maker: Arc<dyn LakeWarehouses>,
    state: Arc<dyn ReplicationStateStore>,
    warehouses: Arc<dyn Warehouses>,
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

/// The two lakes, as a run reaches them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lakes {
    pub source: String,
    pub replica: String,
    pub proxy: String,
}

/// What Settings → Lake says of the replication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicationStatus {
    pub readiness: ReplicationReadiness,
    pub warehouses: Vec<WarehouseState>,
}

/// The host of a base URL, without its scheme, port or path.
fn host_of(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split('/').next()?;
    let host = authority.rsplit_once(':').map_or(authority, |(h, _)| h);
    (!host.is_empty()).then_some(host)
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
    /// With the replication's adapter, the adapter that makes a missing
    /// warehouse (`maker`), the state and the tenants' warehouses.
    pub fn with_replication(
        mut self,
        api: Arc<dyn LakeReplication>,
        maker: Arc<dyn LakeWarehouses>,
        state: Arc<dyn ReplicationStateStore>,
        warehouses: Arc<dyn Warehouses>,
    ) -> Self {
        self.replication = Some(Replication {
            api,
            maker,
            state,
            warehouses,
        });
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
        let Some(host) = host_of(&source) else {
            return Ok(ReplicationReadiness::NoAddress);
        };
        // A pool's nodes each reach their own proxy by the alias they all
        // map to their loopback (0066 step 11, ruling 5); a single node's
        // is its own name.
        let host = match &self.pools {
            Some(pools) if pools.find(MAIN_POOL).await.map_err(backend)?.is_some() => {
                REPLICATION_ALIAS
            }
            _ => host,
        };
        let proxy = format!("{host}:{REPLICATION_PROXY_PORT}");
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
            proxy,
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
        let backend = |e: RepositoryError| LakeAdminError::Backend(e.to_string());
        let Some(source) = self.warehouse_source().await? else {
            return self.replication_status().await;
        };
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
                .warehouse_outcome(replication, &source, &warehouse, lakes, by)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_proxy_is_the_source_nodes_name() {
        assert_eq!(host_of("https://lake-1:9000"), Some("lake-1"));
        assert_eq!(host_of("https://lake-1.internal/"), Some("lake-1.internal"));
        assert_eq!(host_of("lake-1:9000"), Some("lake-1"));
        assert_eq!(host_of("https://"), None);
    }
}
