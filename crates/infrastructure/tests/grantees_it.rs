//! `contracts::Grantees` from the platform's IAM on PostgreSQL (decision
//! 0089, step 5): a custom policy on one pipeline gives that pipeline
//! only, a deny wins, an invited account gets nothing, and a withdrawn
//! policy drops it at the next read. Needs Docker.

use std::sync::Arc;

use application::{PlatformGrantees, PlatformService};
use contracts::{Grantee, Grantees};
use infrastructure::{PgAdminRepository, PgPlatformRepository};
use serde_json::json;
use sqlx::PgPool;

const VIEW: &str = "quality:ViewResults";

async fn account(pool: &PgPool, email: &str, hash: &str) -> String {
    sqlx::query_scalar(
        "INSERT INTO admin_users (email, password_hash, role) VALUES ($1, $2, 'user')
         RETURNING id::text",
    )
    .bind(email)
    .bind(hash)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn policy(pool: &PgPool, name: &str, statements: serde_json::Value, to: &[&str]) {
    sqlx::query("INSERT INTO iam_policies (name, statements) VALUES ($1, $2)")
        .bind(name)
        .bind(statements)
        .execute(pool)
        .await
        .unwrap();
    for who in to {
        sqlx::query(
            "INSERT INTO iam_attachments (principal_kind, principal_id, policy)
             VALUES ('user', $1::uuid, $2)",
        )
        .bind(who)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn each_person_gets_the_pipelines_their_policies_allow() {
    let db = testkit::spawn_test_db().await;
    let pool = db.pool.clone();
    let ana = account(&pool, "ana@example.com", "x").await;
    let bob = account(&pool, "bob@example.com", "x").await;
    let invited = account(&pool, "carla@example.com", "!pending").await;
    let allow = |resources: &[&str]| json!([{ "effect": "Allow", "actions": [VIEW], "resources": resources }]);
    policy(
        &pool,
        "p1-viewers",
        allow(&["pipeline/p1"]),
        &[&ana, &invited],
    )
    .await;
    policy(&pool, "all-viewers", allow(&["pipeline/*"]), &[&bob]).await;
    let deny = json!([{ "effect": "Deny", "actions": [VIEW], "resources": ["pipeline/p2"] }]);
    policy(&pool, "no-p2", deny, &[&bob]).await;

    let platform = PlatformService::new(Arc::new(PgPlatformRepository::new(pool.clone())));
    let grantees = PlatformGrantees::new(
        Arc::new(PgAdminRepository::new(pool.clone())),
        platform.clone(),
    );
    let pipelines: Vec<String> = ["pipeline/p1", "pipeline/p2", "pipeline/p3"]
        .map(String::from)
        .to_vec();
    let mut found = grantees.allowed(VIEW, &pipelines).await.unwrap();
    found.sort_by(|a, b| a.account.cmp(&b.account));
    let mut want = vec![
        Grantee {
            account: ana.clone(),
            resources: vec!["pipeline/p1".into()],
        },
        Grantee {
            account: bob.clone(),
            resources: vec!["pipeline/p1".into(), "pipeline/p3".into()],
        },
    ];
    want.sort_by(|a, b| a.account.cmp(&b.account));
    assert_eq!(
        found, want,
        "one pipeline only, the deny wins, the invited none"
    );

    sqlx::query("DELETE FROM iam_attachments WHERE policy = 'p1-viewers'")
        .execute(&pool)
        .await
        .unwrap();
    platform.invalidate_permissions();
    let found = grantees.allowed(VIEW, &pipelines).await.unwrap();
    assert!(
        found.iter().all(|g| g.account != ana),
        "withdrawn at the next read"
    );
    drop(db);
}
