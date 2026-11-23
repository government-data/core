//! What the replication left of each warehouse, on Postgres
//! (`lake_replication`, 0066 step 11), connected as the application's role
//! so row-level security applies. Needs Docker.

use std::time::{Duration, SystemTime};

use auth_gateway::{Lake, ReplicationOutcome, ReplicationStateStore, ReplicationStep};
use infrastructure::PgReplicationState;
use sqlx::postgres::PgPoolOptions;

const MAIN: &str = "00000000-0000-0000-0000-000000000001";
const A: &str = "qd-tenant-0000000000000000000000000000000a";
const B: &str = "qd-tenant-0000000000000000000000000000000b";

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + secs)
}

fn tenant(id: &str) -> tenancy::Tenant {
    tenancy::Tenant {
        id: id.into(),
        slug: "t".into(),
    }
}

#[tokio::test]
async fn each_warehouse_keeps_its_last_outcome_and_when_it_was_first_configured() {
    let db = testkit::spawn_test_db().await;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(testkit::app_role(&db).await)
        .await
        .unwrap();
    let store = PgReplicationState::new(pool);
    let failed = ReplicationOutcome::Failed {
        step: ReplicationStep::Rule,
        detail: "HTTP 403 AccessDenied".into(),
    };
    tenancy::within(tenant(MAIN), async {
        assert!(store.all().await.unwrap().is_empty());
        let first = store
            .record(B, &ReplicationOutcome::Configured, at(0))
            .await;
        assert_eq!(first.unwrap().configured_at, Some(at(0)));
        let keys = ReplicationOutcome::NoKeys { lake: Lake::main() };
        store.record(A, &keys, at(1)).await.unwrap();
        let later = store.record(B, &failed, at(60)).await.unwrap();
        assert_eq!(
            (later.configured_at, later.checked_at),
            (Some(at(0)), at(60))
        );
        let all = store.all().await.unwrap();
        let read: Vec<_> = all
            .iter()
            .map(|s| (s.warehouse.as_str(), s.outcome.clone()))
            .collect();
        assert_eq!(read, [(A, keys), (B, failed.clone())]);
        assert_eq!(all[0].configured_at, None);
    })
    .await;
}
