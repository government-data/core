//! Isolation between tenants with Row-Level Security.
//!
//! The tests connect as in production: a role without superuser or BYPASSRLS
//! that **owns** the tables (the owner skips RLS if the table has no FORCE).
//! The container's superuser only sets the scene. Needs Docker. The
//! policies stay until the tenants' contraction (decision 0076).

use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres, Transaction};

const MAIN: &str = "00000000-0000-0000-0000-000000000001";

async fn pool_de_la_app(db: &testkit::TestDb) -> PgPool {
    PgPoolOptions::new()
        .connect_with(testkit::app_role(db).await)
        .await
        .unwrap()
}

/// A transaction inside a tenant.
async fn en(pool: &PgPool, tenant: &str) -> Transaction<'static, Postgres> {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(tenant)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx
}

/// A second tenant, for the policies' tests; returns its id.
async fn new_tenant(db: &testkit::TestDb, slug: &str) -> String {
    sqlx::query_scalar("INSERT INTO tenants (slug, name) VALUES ($1, $1) RETURNING id::text")
        .bind(slug)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

async fn iam_groups(tx: &mut Transaction<'static, Postgres>) -> Vec<String> {
    sqlx::query_scalar("SELECT name FROM iam_groups ORDER BY name")
        .fetch_all(&mut **tx)
        .await
        .unwrap()
}

#[tokio::test]
async fn each_tenant_only_sees_and_writes_its_own_rows() {
    let db = testkit::spawn_test_db().await;
    let app = pool_de_la_app(&db).await;
    let acme = new_tenant(&db, "acme").await;

    // Without a tenant set (on-premise) everything goes to the main one.
    sqlx::query("INSERT INTO iam_groups (name) VALUES ('finance'), ('sales')")
        .execute(&app)
        .await
        .unwrap();

    // The same name in another tenant does not clash.
    let mut tx = en(&app, &acme).await;
    sqlx::query("INSERT INTO iam_groups (name) VALUES ('finance')")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(iam_groups(&mut tx).await, ["finance"]);

    // Neither updating nor deleting reaches another tenant's rows.
    let changed = sqlx::query("UPDATE iam_groups SET description = 'acme'")
        .execute(&mut *tx)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(changed, 1);
    tx.commit().await.unwrap();

    let mut tx = en(&app, MAIN).await;
    assert_eq!(iam_groups(&mut tx).await, ["finance", "sales"]);
    let descriptions: Vec<String> =
        sqlx::query_scalar("SELECT description FROM iam_groups WHERE description = 'acme'")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert!(descriptions.is_empty());
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_tenant_cannot_write_rows_for_another() {
    let db = testkit::spawn_test_db().await;
    let app = pool_de_la_app(&db).await;
    let acme = new_tenant(&db, "acme").await;

    let mut tx = en(&app, &acme).await;
    let error =
        sqlx::query("INSERT INTO iam_groups (name, tenant_id) VALUES ('intruder', $1::uuid)")
            .bind(MAIN)
            .execute(&mut *tx)
            .await
            .unwrap_err();
    let code = error.as_database_error().and_then(|e| e.code()).unwrap();
    // insufficient_privilege: the policy rejects it (WITH CHECK).
    assert_eq!(code, "42501");
}

/// A new table with `tenant_id` and without forced RLS would be a hole: this
/// test fails until the migration gives it its policy.
#[tokio::test]
async fn every_tenant_table_has_forced_row_level_security() {
    let db = testkit::spawn_test_db().await;
    let without_rls: Vec<String> = sqlx::query_scalar(
        "SELECT format('%I.%I', n.nspname, c.relname)
           FROM pg_class c
           JOIN pg_namespace n ON n.oid = c.relnamespace
           JOIN pg_attribute a ON a.attrelid = c.oid AND a.attname = 'tenant_id'
          WHERE c.relkind = 'r'
            AND (NOT c.relrowsecurity OR NOT c.relforcerowsecurity
                 OR NOT EXISTS (SELECT 1 FROM pg_policy p WHERE p.polrelid = c.oid))",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert!(
        without_rls.is_empty(),
        "tables with tenant_id and no forced RLS: {without_rls:?}"
    );
}
