//! The machines' active alerts on Postgres (`machine_alerts`, decision 0072
//! step 5), connected as the application's role so row-level security
//! applies. Needs Docker.

use std::time::{Duration, SystemTime};

use auth_gateway::{
    AlertChange, AlertKey, AlertSeverity, FiringAlert, MachineAlertStore, MachineId,
};
use infrastructure::PgMachineAlerts;
use serde_json::{json, Map, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + secs)
}

fn values(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap()
}

fn disk(machine: &str, free: f64) -> FiringAlert {
    FiringAlert {
        key: AlertKey::Machine {
            machine: MachineId::new(machine),
            fingerprint: "0a1b2c3d4e5f6789".into(),
        },
        fingerprint: "0a1b2c3d4e5f6789".into(),
        qd_alert: "disk_low".into(),
        severity: Some(AlertSeverity::Warning),
        starts_at: at(0),
        values: values(json!({ "A": free, "C": 1, "path": "drive1" })),
    }
}

fn unavailable() -> FiringAlert {
    FiringAlert {
        key: AlertKey::Machineless {
            alertname: "DatasourceError".into(),
            rule_name: "disk_low".into(),
        },
        fingerprint: "ffee000011112222".into(),
        qd_alert: "disk_low".into(),
        severity: Some(AlertSeverity::Warning),
        starts_at: at(5),
        values: Map::new(),
    }
}

/// Every row, as (machine, alertname, fingerprint, severity, received_at
/// in seconds after `at(0)`, values).
type Row = (
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    i64,
    Value,
);

async fn rows(pool: &PgPool) -> Vec<Row> {
    async {
        sqlx::query_as(
            "SELECT machine_id::text, alertname, fingerprint, severity, \
               (extract(epoch FROM received_at) - 1800000000)::bigint, \"values\" \
             FROM machine_alerts ORDER BY alertname COLLATE \"C\" NULLS FIRST, fingerprint",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }
    .await
}

async fn world() -> (testkit::TestDb, PgPool, PgMachineAlerts, String) {
    let db = testkit::spawn_test_db().await;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(testkit::app_role(&db).await)
        .await
        .unwrap();
    let machine: (String,) = async {
        sqlx::query_as(
            "INSERT INTO machines (name, role, state) VALUES ('lake-a', 'lake', 'approved') \
             RETURNING id::text",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    }
    .await;
    let store = PgMachineAlerts::new(pool.clone());
    (db, pool, store, machine.0)
}

async fn apply(store: &PgMachineAlerts, changes: &[AlertChange], secs: u64) {
    store.apply(changes, at(secs)).await.unwrap();
}

#[tokio::test]
async fn a_firing_alert_is_kept_once_changed_in_place_and_deleted_when_resolved() {
    let (_db, pool, store, machine) = world().await;
    apply(&store, &[AlertChange::Firing(disk(&machine, 0.07))], 10).await;
    let first = rows(&pool).await;
    let kept = (Some(machine.clone()), None, "0a1b2c3d4e5f6789".to_string());
    assert_eq!(first.len(), 1);
    assert_eq!(
        (first[0].0.clone(), first[0].1.clone(), first[0].2.clone()),
        kept
    );
    assert_eq!((first[0].3.as_deref(), first[0].4), (Some("warning"), 10));
    assert_eq!(first[0].5, json!({ "A": 0.07, "C": 1, "path": "drive1" }));

    apply(&store, &[AlertChange::Firing(disk(&machine, 0.07))], 70).await;
    assert_eq!(
        rows(&pool).await,
        first,
        "the same alert again changes nothing"
    );

    apply(&store, &[AlertChange::Firing(disk(&machine, 0.06))], 130).await;
    let changed = rows(&pool).await;
    assert_eq!((changed.len(), changed[0].4), (1, 130), "changed in place");
    assert_eq!(changed[0].5["A"], json!(0.06));

    let key = disk(&machine, 0.0).key;
    apply(&store, &[AlertChange::Resolved(key.clone())], 190).await;
    assert!(rows(&pool).await.is_empty(), "resolved: deleted");
    apply(&store, &[AlertChange::Resolved(key)], 250).await;
    assert!(rows(&pool).await.is_empty(), "resolved again: nothing");
}

#[tokio::test]
async fn machineless_rows_and_the_heartbeat_are_kept_by_their_names() {
    let (_db, pool, store, machine) = world().await;
    let beat = |fingerprint: &str| AlertChange::Heartbeat {
        fingerprint: fingerprint.into(),
        starts_at: at(1),
    };
    let both = [AlertChange::Firing(unavailable()), beat("aaaa0000bbbb1111")];
    apply(&store, &both, 10).await;
    apply(&store, &both, 70).await;
    let kept = rows(&pool).await;
    let names: Vec<_> = kept
        .iter()
        .map(|r| (r.0.clone(), r.1.clone(), r.4))
        .collect();
    let datasource = (None, Some("DatasourceError".to_string()), 10);
    let heartbeat = (None, Some("alerting_heartbeat".to_string()), 70);
    assert_eq!(names, [datasource, heartbeat], "one row each");
    assert_eq!(kept[1].3, None, "the heartbeat has no severity");

    apply(&store, &[beat("cccc0000dddd2222")], 130).await;
    let beats: Vec<_> = rows(&pool)
        .await
        .into_iter()
        .filter(|r| r.3.is_none())
        .collect();
    assert_eq!(
        (beats.len(), beats[0].2.as_str(), beats[0].4),
        (1, "cccc0000dddd2222", 130)
    );

    // A machine's alert with the same fingerprint is another row.
    let mut same = disk(&machine, 0.07);
    same.key = AlertKey::Machine {
        machine: MachineId::new(&machine),
        fingerprint: "ffee000011112222".into(),
    };
    same.fingerprint = "ffee000011112222".into();
    apply(&store, &[AlertChange::Firing(same)], 190).await;
    assert_eq!(rows(&pool).await.len(), 3);

    let key = unavailable().key;
    apply(&store, &[AlertChange::Resolved(key)], 250).await;
    let left: Vec<_> = rows(&pool).await.into_iter().map(|r| r.1).collect();
    assert_eq!(left, [None, Some("alerting_heartbeat".to_string())]);
}

#[tokio::test]
async fn a_delivery_is_one_transaction() {
    let (_db, pool, store, machine) = world().await;
    let unknown = disk("00000000-0000-0000-0000-00000000dead", 0.07);
    let changes = [
        AlertChange::Firing(disk(&machine, 0.07)),
        AlertChange::Firing(unknown),
    ];
    let failed = store.apply(&changes, at(10)).await;
    assert!(failed.is_err(), "no such machine");
    assert!(rows(&pool).await.is_empty(), "nothing of it kept");
}
