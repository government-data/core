//! `core.iam.changed` (quality-sdk decision 0017, step 4): every statement
//! that changes a policy, a group, its members or an attachment stores one
//! event of the core's topic in the same transaction, naming only the kind
//! of change, and notifies it; a change rolled back stores none. Needs
//! Docker.

use serde_json::{json, Value};
use sqlx::postgres::PgListener;
use sqlx::PgPool;

async fn kinds(pool: &PgPool) -> Vec<(String, i32, String, Value)> {
    sqlx::query_as("SELECT topic, version, publisher, data FROM module_events ORDER BY seq")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn run(pool: &PgPool, sql: &str) {
    sqlx::query(sql).execute(pool).await.unwrap();
}

#[tokio::test]
async fn an_iam_change_publishes_core_iam_changed_once_per_statement() {
    let db = testkit::spawn_test_db().await;
    let pool = db.pool.clone();
    let mut listener = PgListener::connect_with(&pool).await.unwrap();
    listener.listen("module_events").await.unwrap();

    run(
        &pool,
        "INSERT INTO iam_policies (name, statements) VALUES ('readers', '[]'), ('writers', '[]')",
    )
    .await;
    run(&pool, "INSERT INTO iam_groups (name) VALUES ('analysts')").await;
    run(
        &pool,
        "INSERT INTO iam_attachments (principal_kind, principal_id, policy)
         SELECT 'group', id, 'readers' FROM iam_groups WHERE name = 'analysts'",
    )
    .await;
    run(&pool, "DELETE FROM iam_policies WHERE name = 'writers'").await;
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE iam_groups SET description = 'x'")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    let event = |kind: &str| {
        (
            "core.iam.changed".to_owned(),
            1,
            "core".to_owned(),
            json!({ "kind": kind }),
        )
    };
    assert_eq!(
        kinds(&pool).await,
        [
            event("policy"),
            event("group"),
            event("attachment"),
            event("policy")
        ],
        "one per statement, none for the one rolled back"
    );
    let notice = listener.recv().await.unwrap();
    assert_eq!(notice.payload(), "core.iam.changed");
    drop(db);
}
