//! What the contraction migrations dropped is gone after every migration ran,
//! and running the migrations again changes nothing. Needs Docker.

use testkit::spawn_test_db;

async fn exists(pool: &sqlx::PgPool, sql: &str) -> bool {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn the_license_table_and_the_payments_schema_are_gone() {
    let db = spawn_test_db().await;
    infrastructure::run_migrations(&db.pool)
        .await
        .expect("the migrations run twice");

    let license = "SELECT to_regclass('public.orq_license') IS NOT NULL";
    let payments = "SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = 'payments')";
    assert!(!exists(&db.pool, license).await);
    assert!(!exists(&db.pool, payments).await);
}
