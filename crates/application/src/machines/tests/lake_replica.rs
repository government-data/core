//! The replica as Settings → Lake shows it (0066 step 11): the machine of
//! role `lake-replica` that is approved or draining, asked by its name with
//! the keys only the lake's adapter reads.

use auth_gateway::{Lake, LakeAdmin, LakeProbe};

use super::*;

/// A lake's admin API that answers `probe` and records whom it was asked.
struct Api {
    probe: LakeProbe,
    asked: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl LakeAdmin for Api {
    async fn probe(&self, lake: &Lake, node: &str) -> Result<LakeProbe, RepositoryError> {
        self.asked.lock().unwrap().push((lake.key(), node.into()));
        Ok(self.probe.clone())
    }
}

fn answering(probe: LakeProbe) -> (World, Arc<Api>) {
    let api = Arc::new(Api {
        probe,
        asked: Mutex::default(),
    });
    let w = world();
    let w = World {
        service: w.service.with_lake_admin(api.clone()),
        audit: w.audit,
        clock: w.clock,
    };
    (w, api)
}

impl World {
    async fn replica(&self, name: &str) -> Machine {
        let (s, by, from) = (&self.service, admin(), origin());
        let issued = s.issue_join_token(MachineRole::LakeReplica, None, &by, &from);
        let token = issued.await.unwrap().token;
        s.redeem(&token, name, &from).await.unwrap()
    }
}

#[tokio::test]
async fn the_replica_is_its_approved_machine_asked_by_name() {
    let answered = LakeProbe::Answered {
        drives_online: 2,
        drives_offline: 0,
    };
    let (w, api) = answering(answered.clone());
    let s = &w.service;
    let none = ReplicaState::NoMachine { pending: None };
    assert_eq!(s.replica_state().await, Ok(none));
    w.joined("worker-1").await;
    let replica = w.replica("replica-1").await;
    let pending = Some("replica-1".to_string());
    let waiting = ReplicaState::NoMachine { pending };
    assert_eq!(s.replica_state().await, Ok(waiting));
    assert!(api.asked.lock().unwrap().is_empty());
    s.approve(&replica.id, &admin(), &origin()).await.unwrap();
    let probed = ReplicaState::Probed {
        machine: "replica-1".into(),
        probe: answered.clone(),
    };
    assert_eq!(s.replica_state().await, Ok(probed.clone()));
    s.drain(&replica.id, &admin(), &origin()).await.unwrap();
    assert_eq!(s.replica_state().await, Ok(probed));
    let asked = api.asked.lock().unwrap().clone();
    let once = ("replica".to_string(), "replica-1".to_string());
    assert_eq!(asked, vec![once.clone(), once]);
    // A read: nothing audited beyond the machines' own changes.
    let kinds = w.kinds();
    assert!(
        !kinds.contains(&AuditEventType::SecretsChanged),
        "{kinds:?}"
    );
}

#[tokio::test]
async fn a_retired_replica_is_none_and_two_are_not_chosen_between() {
    let refused = LakeProbe::KeysRefused {
        code: "SignatureDoesNotMatch".into(),
    };
    let (w, api) = answering(refused);
    let s = &w.service;
    let first = w.replica("replica-1").await;
    s.approve(&first.id, &admin(), &origin()).await.unwrap();
    s.retire(&first.id, &admin(), &origin()).await.unwrap();
    let none = ReplicaState::NoMachine { pending: None };
    assert_eq!(s.replica_state().await, Ok(none));
    for name in ["replica-2", "replica-3"] {
        let m = w.replica(name).await;
        s.approve(&m.id, &admin(), &origin()).await.unwrap();
    }
    let names = vec!["replica-2".to_string(), "replica-3".to_string()];
    let several = ReplicaState::SeveralMachines { names };
    assert_eq!(s.replica_state().await, Ok(several));
    assert!(api.asked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn without_the_lakes_admin_api_the_state_is_unavailable() {
    let w = world();
    let state = w.service.replica_state().await;
    assert_eq!(state, Err(LakeAdminError::Unavailable));
}

#[tokio::test]
async fn the_main_lake_is_asked_where_the_application_reaches_the_lake() {
    let answered = LakeProbe::Answered {
        drives_online: 4,
        drives_offline: 0,
    };
    let (w, api) = answering(answered.clone());
    let s = w.service.clone();
    assert_eq!(s.main_lake_state().await, Ok(MainLakeState::NoAddress));
    assert!(api.asked.lock().unwrap().is_empty());
    let blank = s.clone().with_main_lake(Some("  ".into()));
    assert_eq!(blank.main_lake_state().await, Ok(MainLakeState::NoAddress));
    let s = s.with_main_lake(Some("https://lake-1:9000/".into()));
    let probed = MainLakeState::Probed {
        endpoint: "https://lake-1:9000".into(),
        probe: answered,
    };
    assert_eq!(s.main_lake_state().await, Ok(probed));
    let asked = api.asked.lock().unwrap().clone();
    assert_eq!(asked, [("pool:main".into(), "https://lake-1:9000".into())]);
    assert!(w.kinds().is_empty());
    let bare = world()
        .service
        .with_main_lake(Some("https://lake-1:9000".into()));
    assert_eq!(
        bare.main_lake_state().await,
        Err(LakeAdminError::Unavailable)
    );
}
