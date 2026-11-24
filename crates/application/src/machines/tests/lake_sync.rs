//! Whether the lake's replica is in sync, as the update agent asks it
//! (decision 0078's amendment): the replica, both lakes' health and each
//! warehouse's replication, read and nothing changed.

use auth_gateway::Lake;

use super::lake_replication::{run, A};
use crate::lake::{LakeHealth, ReplicationState, ReplicationSync};
use crate::machines::ReplicaSync;

fn lake() -> Vec<String> {
    vec!["lake".to_string()]
}

fn synced(pending: u64, failed: u64) -> Option<ReplicationSync> {
    Some(ReplicationSync {
        state: ReplicationState::Configured,
        pending: Some(pending),
        failed: Some(failed),
    })
}

#[tokio::test]
async fn in_sync_takes_a_replica_both_lakes_healthy_and_nothing_waiting_or_failed() {
    let r = run(Some("https://lake-1:9000"), vec![A]);
    let s = &r.w.service;
    let answered = |offline| LakeHealth::Answered {
        drives_online: 4,
        drives_offline: offline,
    };
    *r.lakes.health.lock().unwrap() = Some(answered(0));
    *r.lakes.synced.lock().unwrap() = synced(0, 0);
    assert_eq!(s.replica_sync(&lake()).await, ReplicaSync::NoReplica);
    r.replica("replica-1").await;
    r.keep(Lake::main());
    r.keep(Lake::Replica);
    assert_eq!(s.replica_sync(&lake()).await, ReplicaSync::InSync);
    assert_eq!(
        s.replica_sync(&["query".into()]).await,
        ReplicaSync::Unknown
    );
    *r.lakes.synced.lock().unwrap() = synced(2, 0);
    assert_eq!(s.replica_sync(&lake()).await, ReplicaSync::Behind);
    *r.lakes.synced.lock().unwrap() = synced(2, 1);
    assert_eq!(s.replica_sync(&lake()).await, ReplicaSync::Failed);
    *r.lakes.health.lock().unwrap() = Some(answered(1));
    assert_eq!(s.replica_sync(&lake()).await, ReplicaSync::Unhealthy);
    *r.lakes.health.lock().unwrap() = None;
    assert_eq!(
        s.replica_sync(&lake()).await,
        ReplicaSync::Unhealthy,
        "not reached"
    );
    assert!(r.events("lake:").is_empty(), "a read: nothing audited");
}
