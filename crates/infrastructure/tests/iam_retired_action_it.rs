//! Custom policies that name the retired `admin:ManageLicense` (decision
//! 0074, step 3b): migration 0021 takes the name out of every tenant's
//! policies, run as the tables' owner, which row-level security would
//! otherwise stop. Needs Docker.

use serde_json::{json, Value};
use sqlx::PgPool;

const MIGRATION: &str = include_str!("../migrations/0021_iam_retire_manage_license.sql");

async fn insert(pool: &PgPool, tenant: &str, name: &str, statements: Value) {
    sqlx::query("INSERT INTO iam_policies (tenant_id, name, statements) VALUES ($1::uuid, $2, $3)")
        .bind(tenant)
        .bind(name)
        .bind(statements)
        .execute(pool)
        .await
        .unwrap();
}

async fn statements(pool: &PgPool, tenant: &str, name: &str) -> Value {
    sqlx::query_scalar(
        "SELECT statements FROM iam_policies WHERE tenant_id = $1::uuid AND name = $2",
    )
    .bind(tenant)
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn allow(actions: &[&str], resources: &[&str]) -> Value {
    json!({ "effect": "Allow", "actions": actions, "resources": resources })
}

#[tokio::test]
async fn the_retired_action_leaves_the_policies() {
    let db = testkit::spawn_test_db().await;
    let main = "00000000-0000-0000-0000-000000000001";
    let ops = json!([
        allow(&["admin:ViewAudit", "admin:ManageLicense"], &["*"]),
        allow(&["admin:ManageLicense"], &["*"]),
        allow(&["quality:ViewRules"], &["catalog/*"]),
    ]);
    insert(&db.pool, main, "ops", ops).await;
    let untouched = json!([allow(&["admin:*"], &["*"])]);
    insert(&db.pool, main, "admins", untouched.clone()).await;

    let owner = PgPool::connect_with(testkit::app_role(&db).await)
        .await
        .unwrap();
    for _ in 0..2 {
        sqlx::raw_sql(MIGRATION).execute(&owner).await.unwrap();
    }

    let kept = json!([
        allow(&["admin:ViewAudit"], &["*"]),
        allow(&["quality:ViewRules"], &["catalog/*"]),
    ]);
    assert_eq!(statements(&db.pool, main, "ops").await, kept);
    assert_eq!(statements(&db.pool, main, "admins").await, untouched);

    let forced: bool = sqlx::query_scalar(
        "SELECT relforcerowsecurity FROM pg_class WHERE relname = 'iam_policies'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(forced, "row-level security is forced again");
}
