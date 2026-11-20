//! A lake's pools in Settings → Lake (0066 step 11, the user's rulings 1
//! to 3 of 2026-11-14): `main` defined before any node, again in place
//! until one joins; a node's token names it and joining under another name
//! is refused, the token left unused.

use auth_gateway::{Lake, LakePool, LakePoolStore, PoolLayout, PoolRefusal};

use super::*;

#[derive(Default)]
struct Pools(Mutex<Vec<LakePool>>);

#[async_trait]
impl LakePoolStore for Pools {
    async fn define(
        &self,
        layout: &PoolLayout,
        by: &AdminId,
        at: SystemTime,
    ) -> Result<LakePool, RepositoryError> {
        let pool = LakePool {
            layout: layout.clone(),
            defined_by: by.clone(),
            defined_at: at,
        };
        let mut all = self.0.lock().unwrap();
        all.retain(|p| p.layout.name() != layout.name());
        all.push(pool.clone());
        Ok(pool)
    }

    async fn find(&self, name: &str) -> Result<Option<LakePool>, RepositoryError> {
        let all = self.0.lock().unwrap();
        Ok(all.iter().find(|p| p.layout.name() == name).cloned())
    }

    async fn all(&self) -> Result<Vec<LakePool>, RepositoryError> {
        Ok(self.0.lock().unwrap().clone())
    }
}

fn with_pools() -> World {
    let w = world();
    World {
        service: w.service.with_lake_pools(Arc::new(Pools::default())),
        audit: w.audit,
        clock: w.clock,
    }
}

fn nodes(n: &[&str]) -> Vec<String> {
    n.iter().map(|s| s.to_string()).collect()
}

const FOUR: [&str; 4] = ["lake-a", "lake-b", "lake-c", "lake-d"];

impl World {
    fn actions(&self) -> Vec<String> {
        let events = self.audit.0.lock().unwrap();
        events.iter().map(|e| e.action.clone()).collect()
    }
}

#[tokio::test]
async fn main_is_defined_before_any_node_and_again_until_one_joins() {
    let w = with_pools();
    let s = &w.service;
    let (by, from) = (admin(), origin());
    assert_eq!(s.lake_pools().await, Ok(vec![]));
    let three = s
        .define_pool("main", &nodes(&FOUR[..3]), 1, &by, &from)
        .await;
    assert_eq!(three, Err(PoolError::Layout(PoolRefusal::TooFewNodes)));
    let second = s.define_pool("more", &nodes(&FOUR), 1, &by, &from).await;
    assert_eq!(second, Err(PoolError::NotMain));
    assert!(w.actions().is_empty(), "nothing audited for a refusal");

    let defined = s.define_pool("main", &nodes(&FOUR), 2, &by, &from).await;
    let defined = defined.unwrap();
    assert_eq!(defined.pool.layout.drives(), 2);
    let seen: Vec<_> = defined
        .nodes
        .iter()
        .map(|(n, m)| (n.as_str(), m.is_some()))
        .collect();
    assert_eq!(seen, FOUR.map(|n| (n, false)));
    let moved = nodes(&["lake-e", "lake-a", "lake-b", "lake-c"]);
    let again = s.define_pool("main", &moved, 1, &by, &from);
    let again = again.await.unwrap();
    assert_eq!(again.pool.layout.nodes()[0], "lake-e", "in place, in order");
    assert_eq!(w.actions(), ["lake:pool:defined", "lake:pool:defined"]);
    let event = w.audit.0.lock().unwrap()[0].clone();
    assert_eq!(
        (event.resource.as_str(), event.admin_id),
        ("lake/pool:main", Some(by.clone()))
    );

    let (bare, four) = (world(), nodes(&FOUR));
    let none = bare.service.define_pool("main", &four, 1, &by, &from);
    assert_eq!(none.await, Err(PoolError::Unavailable));
}

#[tokio::test]
async fn a_nodes_token_names_it_and_joining_under_another_name_is_refused() {
    let w = with_pools();
    let s = &w.service;
    let (by, from) = (admin(), origin());
    let undefined = s.issue_node_token("main", "lake-b", 0, &by, &from).await;
    assert_eq!(undefined, Err(MachinesError::Invalid("pool")));
    s.define_pool("main", &nodes(&FOUR), 2, &by, &from)
        .await
        .unwrap();
    let stranger = s.issue_node_token("main", "lake-z", 0, &by, &from).await;
    assert_eq!(stranger, Err(MachinesError::Invalid("node")));
    let drives = s.issue_node_token("main", "lake-b", 1, &by, &from).await;
    assert_eq!(drives, Err(MachinesError::Invalid("drives")));

    let issued = s
        .issue_node_token("main", "lake-b", 2, &by, &from)
        .await
        .unwrap();
    assert_eq!(
        (issued.role, issued.pool.as_deref(), issued.node.as_deref()),
        (MachineRole::Lake, Some("main"), Some("lake-b"))
    );
    let other = s.redeem(&issued.token, "lake-a", &from).await;
    assert_eq!(other, Err(JoinError::NodeName));
    let joined = s.redeem(&issued.token, "lake-b", &from).await.unwrap();
    assert_eq!(
        (joined.role, joined.pool.as_deref()),
        (MachineRole::Lake, Some("main"))
    );
    let actions = w.actions();
    assert_eq!(
        &actions[1..],
        ["join:lake/main/lake-b", "join:node-name", "join:lake"]
    );

    let pools = s.lake_pools().await.unwrap();
    let seen: Vec<_> = pools[0]
        .nodes
        .iter()
        .map(|(n, m)| (n.as_str(), m.is_some()))
        .collect();
    assert_eq!(
        seen,
        [
            ("lake-a", false),
            ("lake-b", true),
            ("lake-c", false),
            ("lake-d", false)
        ]
    );
    let redefine = s.define_pool("main", &nodes(&FOUR), 1, &by, &from).await;
    assert_eq!(redefine, Err(PoolError::Joined));
}

#[tokio::test]
async fn a_pools_replication_goes_through_the_alias_every_node_maps_to_itself() {
    let r = super::lake_replication::run(Some("https://lake-a:9000"), vec!["qd-tenant-a"]);
    let service =
        r.w.service
            .clone()
            .with_lake_pools(Arc::new(Pools::default()));
    r.replica("replica-1").await;
    r.keep(Lake::main());
    r.keep(Lake::Replica);
    let (by, from) = (admin(), origin());
    let ready = service.replication_readiness().await.unwrap();
    let ReplicationReadiness::Ready(lakes) = ready else {
        panic!("ready: {ready:?}");
    };
    assert_eq!(lakes.proxy, "lake-a:9100", "a single node's own name");
    service
        .define_pool("main", &nodes(&FOUR), 1, &by, &from)
        .await
        .unwrap();
    let ready = service.replication_readiness().await.unwrap();
    let ReplicationReadiness::Ready(lakes) = ready else {
        panic!("ready: {ready:?}");
    };
    assert_eq!(lakes.proxy, "qd-lake-replication:9100");
}
