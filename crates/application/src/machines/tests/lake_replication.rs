//! The replication into the replica (0066 step 11, the user's rulings of
//! 2026-11-13): run for every tenant's warehouse once both lakes are known
//! and both keys kept, idempotent, its changes of state audited without a
//! key.

use auth_gateway::{
    Lake, LakeAdminKeyStore, LakeAdminKeys, LakeAdminKeysKept, LakeReplication, LakeWarehouses,
    ReplicationOutcome, ReplicationStateStore, ReplicationStep, WarehouseOutcome,
    WarehouseReplication, WarehouseState, Warehouses,
};

use super::*;

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

/// An adapter that answers `outcome` per warehouse and records each plan.
#[derive(Default)]
pub(super) struct Api {
    outcomes: Mutex<Vec<(String, ReplicationOutcome)>>,
    pub(super) asked: Mutex<Vec<WarehouseReplication>>,
}

/// A main lake that makes a missing warehouse and keeps it, or answers
/// `outcomes` per warehouse; it records each one asked, and its source.
#[derive(Default)]
pub(super) struct Maker {
    pub(super) outcomes: Mutex<Vec<(String, WarehouseOutcome)>>,
    pub(super) asked: Mutex<Vec<(String, String)>>,
    there: Mutex<Vec<String>>,
}

#[async_trait]
impl LakeWarehouses for Maker {
    async fn ensure(
        &self,
        source: &str,
        warehouse: &str,
    ) -> Result<WarehouseOutcome, RepositoryError> {
        let asked = (source.to_string(), warehouse.to_string());
        self.asked.lock().unwrap().push(asked);
        let outcomes = self.outcomes.lock().unwrap();
        if let Some((_, o)) = outcomes.iter().find(|(name, _)| name == warehouse) {
            return Ok(o.clone());
        }
        let mut there = self.there.lock().unwrap();
        if there.iter().any(|w| w == warehouse) {
            return Ok(WarehouseOutcome::Present);
        }
        there.push(warehouse.to_string());
        Ok(WarehouseOutcome::Made)
    }
}

#[async_trait]
impl LakeReplication for Api {
    async fn configure(
        &self,
        w: &WarehouseReplication,
    ) -> Result<ReplicationOutcome, RepositoryError> {
        self.asked.lock().unwrap().push(w.clone());
        let outcomes = self.outcomes.lock().unwrap();
        let found = outcomes.iter().find(|(name, _)| *name == w.warehouse);
        Ok(found.map_or(ReplicationOutcome::Configured, |(_, o)| o.clone()))
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
    pub(super) api: Arc<Api>,
    pub(super) maker: Arc<Maker>,
    pub(super) state: Arc<State>,
}

pub(super) fn run(endpoint: Option<&str>, warehouses: Vec<&'static str>) -> Run {
    let (keys, api, maker, state) = (
        Arc::new(Kept::default()),
        Arc::new(Api::default()),
        Arc::new(Maker::default()),
        Arc::new(State::default()),
    );
    let w = world();
    let service = w
        .service
        .with_lake_admin_keys(keys.clone())
        .with_main_lake(endpoint.map(str::to_string))
        .with_replication(
            api.clone(),
            maker.clone(),
            state.clone(),
            Arc::new(Tenants(warehouses)),
        );
    let w = World {
        service,
        audit: w.audit,
        clock: w.clock,
    };
    Run {
        w,
        keys,
        api,
        maker,
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
    assert!(status.warehouses.is_empty() && r.maker.asked.lock().unwrap().is_empty());
    r.keep(Lake::main());
    let status = s.replicate(None).await.unwrap();
    assert_eq!(
        status.readiness,
        ReplicationReadiness::NoKeys {
            lake: Lake::Replica
        }
    );
    assert!(r.api.asked.lock().unwrap().is_empty());
    assert!(r.replication_events().is_empty());
    let made: Vec<_> = status.warehouses.iter().map(|w| &w.outcome).collect();
    assert_eq!(
        made,
        [&ReplicationOutcome::Made],
        "made, its replication waiting"
    );
}

#[tokio::test]
async fn every_warehouse_is_configured_through_the_sources_proxy_and_audited_once() {
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
        proxy: "lake-1:9100".into(),
    };
    assert_eq!(status.readiness, ReplicationReadiness::Ready(lakes));
    let asked = r.api.asked.lock().unwrap().clone();
    let names: Vec<&str> = asked.iter().map(|p| p.warehouse.as_str()).collect();
    assert_eq!(names, [A, B], "each once, in order");
    assert!(asked
        .iter()
        .all(|p| p.proxy == "lake-1:9100" && p.replica == "replica-1"));
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
    assert_eq!(r.api.asked.lock().unwrap().len(), 4);
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
    r.api.outcomes.lock().unwrap().extend([
        (A.to_string(), failed.clone()),
        (B.to_string(), ReplicationOutcome::NoWarehouse),
    ]);
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
