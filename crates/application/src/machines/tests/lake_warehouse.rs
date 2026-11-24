//! A tenant's warehouse made on the main lake (0066 step 11, the user's
//! ruling of 2026-11-13 on the warehouse): by the replication's runs, with
//! the main lake's keys alone, before its replication; audited once when
//! made, never with a key.

use auth_gateway::{Lake, ReplicationOutcome, ReplicationStep};

use crate::lake::{LakeError, WarehouseMade};

use super::lake_replication::{run, A, B};
use super::*;

const SOURCE: &str = "https://lake-1:9000";

fn made_by(w: &str, by: Option<&str>) -> (String, String, Option<String>) {
    (
        "lake:warehouse:made".to_string(),
        format!("lake/pool:main/{w}"),
        by.map(str::to_string),
    )
}

#[tokio::test]
async fn each_warehouse_is_made_with_the_main_lakes_keys_alone_and_audited_once() {
    let r = run(Some(SOURCE), vec![B, A]);
    let s = &r.w.service;
    s.replicate(None).await.unwrap();
    assert!(r.lakes.warehouses_asked().is_empty(), "no keys yet");
    r.keep(Lake::main());
    let (by, from) = (admin(), origin());
    let status = s.replicate(Some((&by, &from))).await.unwrap();
    let none = ReplicaState::NoMachine { pending: None };
    assert_eq!(status.readiness, ReplicationReadiness::NoReplica(none));
    let asked = r.lakes.warehouses_asked();
    let wanted = [A, B].map(|w| (SOURCE.to_string(), w.to_string()));
    assert_eq!(asked, wanted, "each once, in order, at the main lake");
    let states: Vec<_> = status.warehouses.iter().map(|w| &w.outcome).collect();
    assert_eq!(states, [&ReplicationOutcome::Made; 2]);
    assert_eq!(
        r.events("lake:warehouse"),
        [made_by(A, None), made_by(B, None)]
    );
    let e = r.w.audit.0.lock().unwrap().last().cloned().unwrap();
    assert_eq!((e.admin_id, e.ip.as_str()), (Some(by), "192.0.2.10"));
    // The reconcile again: there already, nothing new audited.
    let again = s.replicate(None).await.unwrap();
    assert_eq!(r.lakes.warehouses_asked().len(), 4);
    assert_eq!(r.events("lake:warehouse").len(), 2);
    assert_eq!(again.warehouses.len(), 2);
    assert!(r.lakes.replications_asked().is_empty() && r.replication_events().is_empty());
}

#[tokio::test]
async fn once_the_replica_is_ready_the_made_warehouse_is_replicated_in_the_same_run() {
    let r = run(Some(SOURCE), vec![A]);
    r.keep(Lake::main());
    r.replica("replica-1").await;
    r.keep(Lake::Replica);
    let s = &r.w.service;
    let status = s.replicate(None).await.unwrap();
    assert_eq!(r.lakes.warehouses_asked().len(), 1);
    let asked = r.lakes.replications_asked();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].1, A);
    assert_eq!(status.warehouses[0].outcome, ReplicationOutcome::Configured);
    let reconciler = Some(RECONCILER);
    assert_eq!(r.events("lake:warehouse"), [made_by(A, reconciler)]);
    assert_eq!(r.replication_events().len(), 1);
}

#[tokio::test]
async fn a_warehouse_not_made_is_kept_failed_on_the_main_lake_and_not_replicated() {
    let r = run(Some(SOURCE), vec![A, B]);
    r.keep(Lake::main());
    r.replica("replica-1").await;
    r.keep(Lake::Replica);
    let refused = WarehouseMade::Failed {
        step: "warehouse_tables".into(),
        detail: "HTTP 400".into(),
    };
    let down = LakeError::Unavailable("PUT warehouses: no connection".into());
    r.lakes
        .made
        .lock()
        .unwrap()
        .extend([(A.to_string(), Ok(refused)), (B.to_string(), Err(down))]);
    let s = &r.w.service;
    let status = s.replicate(None).await.unwrap();
    assert!(
        r.lakes.replications_asked().is_empty(),
        "nothing replicated"
    );
    let failed = ReplicationOutcome::Failed {
        step: ReplicationStep::WarehouseTables,
        detail: "HTTP 400".into(),
    };
    let not_reached = ReplicationOutcome::Failed {
        step: ReplicationStep::WarehouseBucket,
        detail: "unavailable: PUT warehouses: no connection".into(),
    };
    let states: Vec<_> = status
        .warehouses
        .iter()
        .map(|w| w.outcome.clone())
        .collect();
    assert_eq!(states, [failed, not_reached]);
    let event = |step: &str, w: &str| {
        (
            format!("lake:warehouse:failed:{step}"),
            format!("lake/pool:main/{w}"),
            Some(RECONCILER.to_string()),
        )
    };
    let events = [event("warehouse_tables", A), event("warehouse_bucket", B)];
    assert_eq!(r.events("lake:"), events);
    s.replicate(None).await.unwrap();
    assert_eq!(r.events("lake:"), events, "not audited twice");
    assert_eq!(r.state.0.lock().unwrap()[0].configured_at, None);
}
