//! What the quality module tells the update agent is still to finish
//! (decision 0003), on an ephemeral PostgreSQL: jobs held by a worker,
//! jobs waiting, and enabled schedules due within the horizon. Needs Docker.

use std::time::Duration;

use contracts::{Drain, ModuleRegistry, Work};
use infrastructure::module_migrations::PgModuleMigrator;
use quality::drain::QualityDrain;
use quality::module::QualityModule;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// A migrated database, and a pool connected as the API's role (not a
/// superuser, so row security applies as it does when serving).
async fn migrated() -> (testkit::TestDb, PgPool) {
    let db = testkit::spawn_test_db().await;
    sqlx::query("CREATE ROLE qd_queue LOGIN PASSWORD 'queue'")
        .execute(&db.pool)
        .await
        .unwrap();
    let mut migrator = PgModuleMigrator::load(&db.pool).await.unwrap();
    ModuleRegistry::new()
        .register(QualityModule::default())
        .run_migrations(&mut migrator)
        .unwrap();
    migrator.run(&db.pool).await.unwrap();
    let options = testkit::app_role(&db).await;
    for sql in [
        "GRANT USAGE ON SCHEMA quality TO app",
        "ALTER TABLE quality.jobs OWNER TO app",
        "ALTER TABLE quality.schedules OWNER TO app",
    ] {
        sqlx::query(sql).execute(&db.pool).await.unwrap();
    }
    let app = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap();
    (db, app)
}

async fn job(pool: &PgPool, state: &str) {
    sqlx::query("INSERT INTO quality.jobs (kind, dataset, state) VALUES ('evaluate', 'd', $1)")
        .bind(state)
        .execute(pool)
        .await
        .unwrap();
}

/// A schedule whose next fire is `in_secs` from now.
async fn schedule(pool: &PgPool, in_secs: i64, enabled: bool) {
    sqlx::query(
        "INSERT INTO quality.schedules (cron, fires, kind, enabled, next_fire_at)
         VALUES ('0 2 * * *', 'job', 'maintain', $1, now() + make_interval(secs => $2))",
    )
    .bind(enabled)
    .bind(in_secs as f64)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn it_counts_jobs_held_waiting_and_schedules_due_within_the_horizon() {
    let (db, app) = migrated().await;
    let drain = QualityDrain::new(app);
    let none = drain.work(Duration::from_secs(600)).await.unwrap();
    assert_eq!(none, Work::default(), "an empty installation has nothing");

    for state in ["running", "running", "queued", "done", "failed"] {
        job(&db.pool, state).await;
    }
    schedule(&db.pool, 60, true).await;
    schedule(&db.pool, 3600, true).await;
    schedule(&db.pool, 60, false).await;

    let work = drain.work(Duration::from_secs(600)).await.unwrap();
    assert_eq!(
        work,
        Work {
            running: 2,
            queued: 1,
            scheduled_soon: 1,
        },
        "the disabled schedule and the one in an hour are not due"
    );
    let wider = drain.work(Duration::from_secs(7200)).await.unwrap();
    assert_eq!(wider.scheduled_soon, 2);
}

#[test]
fn without_a_database_the_module_counts_nothing() {
    assert!(ModuleRegistry::new()
        .register(QualityModule::default())
        .drains()
        .is_empty());
}
