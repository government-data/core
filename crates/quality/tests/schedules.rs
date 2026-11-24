//! The schedules' store on an ephemeral PostgreSQL, as the application's
//! role, whose row security leaves it the main tenant's rows (decision
//! 0035): what it saves is what the scheduler fires; an edit computes the
//! next fire again; a schedule with cuts is not deleted.

use chrono::{DateTime, TimeZone, Utc};
use contracts::ModuleRegistry;
use infrastructure::module_migrations::PgModuleMigrator;
use quality::firing::{self, Outcome};
use quality::module::QualityModule;
use quality::schedule_store::{self, Deleted, ScheduleSpec};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

struct Fixture {
    _db: testkit::TestDb,
    app: PgPool,
}

async fn fixture() -> Fixture {
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
        "ALTER TABLE quality.cuts OWNER TO app",
    ] {
        sqlx::query(sql).execute(&db.pool).await.unwrap();
    }
    let app = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    Fixture { _db: db, app }
}

/// 12:00 UTC on 31 October 2026: 07:00 in Bogotá.
fn noon() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap()
}

fn spec(value: serde_json::Value) -> ScheduleSpec {
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn a_saved_schedule_is_what_the_scheduler_fires_and_an_edit_moves_its_next_fire() {
    let f = fixture().await;
    async {
        let daily = spec(
            json!({ "cron": "0 6 * * *", "fires": "cut", "dataset": "loans", "rerun": "replace" }),
        );
        let first = daily.check(noon()).unwrap();
        let id = schedule_store::create(&f.app, &daily, first).await.unwrap();
        let stored = schedule_store::get(&f.app, id).await.unwrap().unwrap();
        assert_eq!((stored.spec, stored.next_fire_at), (daily, Some(first)));

        // At its time, the scheduler fires it: a cut of loans.
        let fired = firing::fire_due(&f.app, first, 24).await.unwrap();
        assert_eq!((fired.len(), fired[0].outcome), (1, Outcome::Queued));

        // Edited to 18:00: its next fire is computed again, from now.
        let evening = spec(
            json!({ "cron": "0 18 * * *", "fires": "cut", "dataset": "loans", "rerun": "block" }),
        );
        let next = evening.check(noon()).unwrap();
        assert!(schedule_store::update(&f.app, id, &evening, next)
            .await
            .unwrap());
        let stored = schedule_store::get(&f.app, id).await.unwrap().unwrap();
        assert_eq!(
            stored.next_fire_at,
            Some(Utc.with_ymd_and_hms(2026, 10, 31, 23, 0, 0).unwrap())
        );
        assert_eq!(stored.last_outcome.as_deref(), Some("queued"));

        // It has a cut now: disabled, not deleted.
        assert_eq!(
            schedule_store::delete(&f.app, id).await.unwrap(),
            Deleted::HasCuts
        );
        let maintain = spec(
            json!({ "cron": "0 2 * * 0", "fires": "job", "kind": "maintain", "dataset": "loans" }),
        );
        let other = schedule_store::create(&f.app, &maintain, maintain.check(noon()).unwrap())
            .await
            .unwrap();
        assert_eq!(schedule_store::list(&f.app).await.unwrap().len(), 2);
        assert_eq!(
            schedule_store::delete(&f.app, other).await.unwrap(),
            Deleted::Deleted
        );
        assert_eq!(
            schedule_store::delete(&f.app, other).await.unwrap(),
            Deleted::NotFound
        );
    }
    .await;
}

#[tokio::test]
async fn a_trigger_is_saved_as_written_and_once_fires_once() {
    let f = fixture().await;
    async {
        // Tomorrow at 06:00 in Bogotá, only that once; a cron sent along is
        // not kept.
        let once = spec(json!({
            "cron": "0 6 * * *",
            "trigger": { "start": "2026-11-01T06:00:00", "recurrence": { "kind": "once" } },
            "fires": "cut", "dataset": "loans", "rerun": "block",
        }));
        let first = once.check(noon()).unwrap();
        assert_eq!(first, Utc.with_ymd_and_hms(2026, 11, 1, 11, 0, 0).unwrap());
        let id = schedule_store::create(&f.app, &once, first).await.unwrap();
        let stored = schedule_store::get(&f.app, id).await.unwrap().unwrap();
        assert_eq!(stored.spec.trigger, once.trigger);
        assert_eq!(stored.spec.cron, "");

        // It fires its cut, then has no next fire, and is not due again.
        let fired = firing::fire_due(&f.app, first, 24).await.unwrap();
        assert_eq!((fired.len(), fired[0].outcome), (1, Outcome::Queued));
        let stored = schedule_store::get(&f.app, id).await.unwrap().unwrap();
        assert_eq!(stored.next_fire_at, None);
        assert!(stored.spec.enabled);
        let later = first + chrono::Duration::days(7);
        assert!(firing::fire_due(&f.app, later, 24)
            .await
            .unwrap()
            .is_empty());
    }
    .await;
}
