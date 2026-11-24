//! The replication into the replica (0066 step 11, the user's rulings of
//! 2026-11-13): run for every tenant's warehouse once both lakes are known
//! and both keys kept, idempotent, its changes of state audited without a
//! key.

use auth_gateway::{
    Lake, LakeAdminKeyStore, LakeAdminKeys, LakeAdminKeysKept, ReplicationOutcome,
    ReplicationStateStore, ReplicationStep, WarehouseState, Warehouses,
};

use super::lake_adapters::{Adapters, Shared};
use super::*;
use crate::lake::{ReplicaTarget, ReplicationState};

/// Which lakes have keys kept.
#[derive(Default)]
struct Kept(Mutex<Vec<String>>);

#[async_trait]
impl LakeAdminKeyStore for Kept {
    async fn set(
        &self,
        lake: &Lake,
        keys: &LakeAdminKeys,
        by: &AdminId,
        at: SystemTime,
    ) -> Result<LakeAdminKeysKept, RepositoryError> {
        self.0.lock().unwrap().push(lake.key());
        Ok(LakeAdminKeysKept {
            access_key: keys.access_key.clone(),
            set_by: by.clone(),
            set_at: at,
        })
    }

    async fn kept(&self, lake: &Lake) -> Result<Option<LakeAdminKeysKept>, RepositoryError> {
        let kept = self.0.lock().unwrap().contains(&lake.key());
        Ok(kept.then(|| LakeAdminKeysKept {
            access_key: "qd-platform-admin".into(),
            set_by: admin(),
            set_at: SystemTime::UNIX_EPOCH,
        }))
    }
}

#[derive(Default)]
pub(super) struct State(pub(super) Mutex<Vec<WarehouseState>>);

#[async_trait]
impl ReplicationStateStore for State {
    async fn record(
        &self,
        warehouse: &str,
        outcome: &ReplicationOutcome,
        at: SystemTime,
    ) -> Result<WarehouseState, RepositoryError> {
        let mut all = self.0.lock().unwrap();
        let first = all
            .iter()
            .find(|s| s.warehouse == warehouse)
            .and_then(|s| s.configured_at);
        let configured = (*outcome == ReplicationOutcome::Configured).then_some(at);
        all.retain(|s| s.warehouse != warehouse);
        let state = WarehouseState {
            warehouse: warehouse.into(),
            outcome: outcome.clone(),
            configured_at: first.or(configured),
            checked_at: at,
        };
        all.push(state.clone());
        all.sort_by(|a, b| a.warehouse.cmp(&b.warehouse));
        Ok(state)
    }

    async fn all(&self) -> Result<Vec<WarehouseState>, RepositoryError> {
        Ok(self.0.lock().unwrap().clone())
    }
}

struct Tenants(Vec<&'static str>);

#[async_trait]
impl Warehouses for Tenants {
    async fn warehouses(&self) -> Result<Vec<String>, RepositoryError> {
        Ok(self.0.iter().map(|w| w.to_string()).collect())
    }
}

pub(super) struct Run {
    pub(super) w: World,
    keys: Arc<Kept>,
    pub(super) lakes: Arc<Adapters>,
    pub(super) state: Arc<State>,
}

pub(super) fn run(endpoint: Option<&str>, warehouses: Vec<&'static str>) -> Run {
    let (keys, lakes, state) = (
        Arc::new(Kept::default()),
        Arc::new(Adapters::default()),
        Arc::new(State::default()),
    );
    let w = world();
    let service = w
        .service
        .with_lake_admin_keys(keys.clone())
        .with_main_lake(endpoint.map(str::to_string))
        .with_lake_admin(Arc::new(Shared(lakes.clone())))
        .with_replication(state.clone(), Arc::new(Tenants(warehouses)));
    let w = World {
        service,
        audit: w.audit,
        clock: w.clock,
    };
    Run {
        w,
        keys,
        lakes,
        state,
    }
}

impl Run {
    pub(super) async fn replica(&self, name: &str) {
        let (s, by, from) = (&self.w.service, admin(), origin());
        let issued = s.issue_join_token(MachineRole::LakeReplica, None, &by, &from);
        let token = issued.await.unwrap().token;
        let m = s.redeem(&token, name, &from).await.unwrap();
        s.approve(&m.id, &by, &from).await.unwrap();
    }

    pub(super) fn keep(&self, lake: Lake) {
        self.keys.0.lock().unwrap().push(lake.key());
    }

    pub(super) fn replication_events(&self) -> Vec<(String, String, Option<String>)> {
        self.events("lake:replication")
    }

    /// The audit's events whose action starts with `prefix`.
    pub(super) fn events(&self, prefix: &str) -> Vec<(String, String, Option<String>)> {
        let events = self.w.audit.0.lock().unwrap();
        events
            .iter()
            .filter(|e| e.action.starts_with(prefix))
            .map(|e| {
                (
                    e.action.clone(),
                    e.resource.clone(),
                    e.service_account.clone(),
                )
            })
            .collect()
    }
}

pub(super) const A: &str = "qd-tenant-0000000000000000000000000000000a";
pub(super) const B: &str = "qd-tenant-0000000000000000000000000000000b";

#[tokio::test]
async fn nothing_runs_until_both_lakes_are_known_and_both_keys_kept() {
    let r = run(None, vec![A]);
    let s = &r.w.service;
    let status = s.replicate(None).await.unwrap();
    assert_eq!(status.readiness, ReplicationReadiness::NoAddress);
    let r = run(Some("https://lake-1:9000"), vec![A]);
    let s = &r.w.service;
    let none = ReplicaState::NoMachine { pending: None };
    let status = s.replicate(None).await.unwrap();
    assert_eq!(status.readiness, ReplicationReadiness::NoReplica(none));
    r.replica("replica-1").await;
    let status = s.replicate(None).await.unwrap();
    assert_eq!(
        status.readiness,
        ReplicationReadiness::NoKeys { lake: Lake::main() }
    );
    assert!(status.warehouses.is_empty() && r.lakes.warehouses_asked().is_empty());
    r.keep(Lake::main());
    let status = s.replicate(None).await.unwrap();
    assert_eq!(
        status.readiness,
        ReplicationReadiness::NoKeys {
            lake: Lake::Replica
        }
    );
    assert!(r.lakes.replications_asked().is_empty());
    assert!(r.replication_events().is_empty());
    let made: Vec<_> = status.warehouses.iter().map(|w| &w.outcome).collect();
    assert_eq!(
        made,
        [&ReplicationOutcome::Made],
        "made, its replication waiting"
    );
}

#[tokio::test]
async fn every_warehouse_is_configured_by_the_main_lakes_adapter_and_audited_once() {
    let r = run(Some("https://lake-1:9000/"), vec![B, A, A]);
    r.replica("replica-1").await;
    r.keep(Lake::main());
    r.keep(Lake::Replica);
    let s = &r.w.service;
    let (by, from) = (admin(), origin());
    let status = s.replicate(Some((&by, &from))).await.unwrap();
    let lakes = Lakes {
        source: "https://lake-1:9000".into(),
        replica: "replica-1".into(),
        source_pool: false,
    };
    assert_eq!(status.readiness, ReplicationReadiness::Ready(lakes));
    let target = ReplicaTarget {
        endpoint: "replica-1".into(),
        source_pool: false,
    };
    let at = |w: &str| {
        (
            "https://lake-1:9000".to_string(),
            w.to_string(),
            target.clone(),
        )
    };
    assert_eq!(
        r.lakes.replications_asked(),
        [at(A), at(B)],
        "each once, in order, of the main lake"
    );
    assert_eq!(status.warehouses.len(), 2);
    assert!(status
        .warehouses
        .iter()
        .all(|w| w.outcome == ReplicationOutcome::Configured));
    let by_person = |w: &str| {
        (
            "lake:replication:configured".to_string(),
            format!("lake/replica/{w}"),
            None,
        )
    };
    assert_eq!(r.replication_events(), [by_person(A), by_person(B)]);
    let e = r.w.audit.0.lock().unwrap().last().cloned().unwrap();
    assert_eq!((e.admin_id, e.ip.as_str()), (Some(by), "192.0.2.10"));
    // The reconcile again: asked again, nothing new audited.
    r.w.advance(Duration::from_secs(3600));
    let again = s.replicate(None).await.unwrap();
    assert_eq!(r.lakes.replications_asked().len(), 4);
    assert_eq!(r.replication_events().len(), 2);
    let first = status.warehouses[0].configured_at;
    assert_eq!(
        again.warehouses[0].configured_at, first,
        "first configured kept"
    );
    assert_eq!(
        again.warehouses[0].checked_at,
        SystemTime::UNIX_EPOCH + Duration::from_secs(3600)
    );
}

#[tokio::test]
async fn a_failure_is_kept_audited_by_the_application_and_a_missing_warehouse_is_not() {
    let r = run(Some("https://lake-1:9000"), vec![A, B]);
    r.replica("replica-1").await;
    r.keep(Lake::main());
    r.keep(Lake::Replica);
    let failed = ReplicationOutcome::Failed {
        step: ReplicationStep::RemoteTarget,
        detail: "HTTP 502".into(),
    };
    let answered = ReplicationState::Failed {
        step: "remote_target".into(),
        detail: "HTTP 502".into(),
    };
    let unknown = crate::lake::LakeError::Unknown(format!("PUT {B}: no such warehouse"));
    r.lakes
        .replicated
        .lock()
        .unwrap()
        .extend([(A.to_string(), Ok(answered)), (B.to_string(), Err(unknown))]);
    let s = &r.w.service;
    let status = s.replicate(None).await.unwrap();
    let outcomes: Vec<_> = status
        .warehouses
        .iter()
        .map(|w| w.outcome.clone())
        .collect();
    assert_eq!(outcomes, [failed.clone(), ReplicationOutcome::NoWarehouse]);
    let reconciler = Some(RECONCILER.to_string());
    let event = (
        "lake:replication:failed:remote_target".to_string(),
        format!("lake/replica/{A}"),
        reconciler,
    );
    assert_eq!(r.replication_events(), std::slice::from_ref(&event));
    s.replicate(None).await.unwrap();
    assert_eq!(
        r.replication_events(),
        [event],
        "the same failure is not audited twice"
    );
    assert_eq!(r.state.0.lock().unwrap()[0].configured_at, None);
    let status = s.replication_status().await.unwrap();
    assert_eq!(status.warehouses.len(), 2);
}

#[tokio::test]
async fn without_the_replication_its_use_cases_are_unavailable() {
    let w = world();
    assert_eq!(
        w.service.replicate(None).await,
        Err(LakeAdminError::Unavailable)
    );
    assert_eq!(
        w.service.replication_status().await,
        Err(LakeAdminError::Unavailable)
    );
}
