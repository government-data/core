//! The lake's pools on Postgres (`lake_pools`, 0066 step 11), connected as
//! the application's role so row-level security applies. Needs Docker.

use std::time::{Duration, SystemTime};

use auth_gateway::{LakePoolStore, PoolLayout};
use domain::AdminId;
use infrastructure::PgLakePools;
use sqlx::postgres::PgPoolOptions;

const MAIN: &str = "00000000-0000-0000-0000-000000000001";
const ADMIN: &str = "00000000-0000-0000-0000-0000000000aa";
const OTHER: &str = "00000000-0000-0000-0000-0000000000bb";

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + secs)
}

fn tenant(id: &str) -> tenancy::Tenant {
    tenancy::Tenant {
        id: id.into(),
        slug: "t".into(),
    }
}

fn layout(nodes: &[&str], drives: u16) -> PoolLayout {
    let nodes: Vec<String> = nodes.iter().map(|n| n.to_string()).collect();
    PoolLayout::parse("main", &nodes, drives).unwrap()
}

#[tokio::test]
async fn a_pool_is_kept_in_order_defined_again_in_place_and_seen_by_its_tenant_alone() {
    let db = testkit::spawn_test_db().await;
    let acme = testkit::new_tenant(&db, "acme").await;
    let pool = infrastructure::db::with_tenancy(
        PgPoolOptions::new().max_connections(4),
        tenancy::Mode::Multiple,
    )
    .connect_with(testkit::app_role(&db).await)
    .await
    .unwrap();
    let store = PgLakePools::new(pool.clone());
    let first = layout(&["lake-d", "lake-a", "lake-c", "lake-b"], 2);
    let second = layout(&["lake-a", "lake-b", "lake-c", "lake-d", "lake-e"], 1);
    tenancy::within(tenant(MAIN), async {
        assert!(store.find("main").await.unwrap().is_none());
        assert!(store.all().await.unwrap().is_empty());
        let kept = store
            .define(&first, &AdminId::new(ADMIN), at(0))
            .await
            .unwrap();
        assert_eq!(kept.layout, first, "the nodes in the order given");
        assert_eq!((kept.defined_by.as_str(), kept.defined_at), (ADMIN, at(0)));
        let again = store.define(&second, &AdminId::new(OTHER), at(60)).await;
        assert_eq!(again.unwrap().layout, second);
        let found = store.find("main").await.unwrap().unwrap();
        assert_eq!(
            (found.layout, found.defined_by.as_str()),
            (second.clone(), OTHER)
        );
        assert_eq!(store.all().await.unwrap().len(), 1, "one row per pool");
    })
    .await;
    tenancy::within(tenant(&acme), async {
        assert!(store.all().await.unwrap().is_empty(), "another tenant's");
    })
    .await;
    let three: Result<(i64,), _> = tenancy::within(tenant(MAIN), async {
        sqlx::query_as(
            "INSERT INTO lake_pools (name, nodes, drives, defined_by, defined_at) \
             VALUES ('two', ARRAY['a','b','c'], 1, $1::uuid, now()) RETURNING 1::bigint",
        )
        .bind(ADMIN)
        .fetch_one(&pool)
        .await
    })
    .await;
    assert!(
        three.is_err(),
        "fewer than four nodes refused by the table too"
    );
}
