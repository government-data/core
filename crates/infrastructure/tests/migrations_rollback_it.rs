//! Going back to a previous version after an update: the database already has
//! migrations the previous one does not carry, and it still has to start.
//! What is still rejected is a known migration that changed. Needs Docker.

use testkit::spawn_test_db;

/// What a newer version would leave: its recorded migration and what it added
/// (it only adds, as `cargo xtask migrations` requires).
async fn apply_newer_migration(pool: &sqlx::PgPool) {
    sqlx::query("ALTER TABLE system_settings ADD COLUMN de_la_nueva TEXT")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
         VALUES (99990001, 'de la version newer', true, '\\x00'::bytea, 0)",
    )
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn the_previous_version_starts_on_a_database_a_newer_one_migrated() {
    let db = spawn_test_db().await;
    apply_newer_migration(&db.pool).await;

    infrastructure::run_migrations(&db.pool)
        .await
        .expect("the previous version has to start on the migrated database");

    // And what the new one added stays where it was.
    let (known,): (bool,) =
        sqlx::query_as("SELECT EXISTS (SELECT 1 FROM _sqlx_migrations WHERE version = 99990001)")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(known);
}

#[tokio::test]
async fn a_known_migration_that_changed_is_still_rejected() {
    let db = spawn_test_db().await;
    let first = infrastructure::MIGRATOR.iter().next().unwrap().version;
    sqlx::query("UPDATE _sqlx_migrations SET checksum = '\\x00'::bytea WHERE version = $1")
        .bind(first)
        .execute(&db.pool)
        .await
        .unwrap();

    let error = infrastructure::run_migrations(&db.pool)
        .await
        .expect_err("a known migration that changed is not tolerated");
    assert!(
        matches!(error, sqlx::migrate::MigrateError::VersionMismatch(v) if v == first),
        "{error:?}"
    );
}
