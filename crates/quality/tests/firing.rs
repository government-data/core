//! Firing schedules on an ephemeral PostgreSQL, as the application's role,
//! whose row security leaves it the main tenant's rows (decision 0035): a
//! due schedule opens one cut with its extraction, or queues its job, once
//! even with two schedulers; a date that has its cut blocks or replaces it;
//! after an absence only the latest fire runs; only the VM holding the
//! scheduler's lock fires, and another takes over.

use chrono::{DateTime, Duration, TimeZone, Utc};
use contracts::ModuleRegistry;
use infrastructure::module_migrations::PgModuleMigrator;
use quality::firing::{self, Outcome};
use quality::module::QualityModule;
use quality::scheduler::Scheduler;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

/// The installation's tenant, the main one (decision 0076).
const A: &str = "00000000-0000-0000-0000-000000000001";

/// 06:00 in Bogotá on 16 October 2026.
fn six_am() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 16, 11, 0, 0).unwrap()
}

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

fn tenant(id: &str) -> tenancy::Tenant {
    tenancy::Tenant {
        id: id.into(),
        slug: String::new(),
    }
}

impl Fixture {
    /// Runs `sql` as `t`, returning its first column as text if any.
    async fn as_tenant(&self, t: &str, sql: &str) -> Vec<String> {
        tenancy::within(tenant(t), async {
            sqlx::query_scalar(sql).fetch_all(&self.app).await.unwrap()
        })
        .await
    }

    async fn schedule(&self, t: &str, values: &str, due: DateTime<Utc>) -> Uuid {
        tenancy::within(tenant(t), async {
            sqlx::query_scalar(&format!(
                "INSERT INTO quality.schedules (cron, fires, dataset, rerun, kind, next_fire_at)
                 VALUES ({values}, $1) RETURNING id"
            ))
            .bind(due)
            .fetch_one(&self.app)
            .await
            .unwrap()
        })
        .await
    }

    async fn fire(&self, t: &str, now: DateTime<Utc>, hours: u32) -> Vec<firing::Fired> {
        tenancy::within(tenant(t), firing::fire_due(&self.app, now, hours))
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn a_due_cut_opens_once_with_its_extraction_even_with_two_schedulers() {
    let f = fixture().await;
    let id = f
        .schedule(A, "'0 6 * * *', 'cut', 'clientes', 'block', NULL", six_am())
        .await;
    let now = six_am() + Duration::seconds(10);
    let (one, two) = tokio::join!(f.fire(A, now, 24), f.fire(A, now, 24));
    let fired: Vec<_> = one.into_iter().chain(two).collect();
    assert_eq!(fired.len(), 1, "{fired:?}");
    assert_eq!(
        (fired[0].schedule, fired[0].outcome, fired[0].fire),
        (id, Outcome::Queued, Some(six_am()))
    );
    assert_eq!(
        f.as_tenant(A, "SELECT cut::text || ' ' || state FROM quality.cuts")
            .await,
        ["2026-10-16 extracting"]
    );
    assert_eq!(
        f.as_tenant(
            A,
            "SELECT j.kind || ' ' || (j.parameters->>'cut') FROM quality.jobs j
               JOIN quality.cuts c ON c.extract_job = j.id"
        )
        .await,
        ["extract 2026-10-16"]
    );
    assert_eq!(
        f.as_tenant(
            A,
            "SELECT next_fire_at::text || ' ' || last_outcome FROM quality.schedules"
        )
        .await,
        ["2026-10-17 11:00:00+00 queued"]
    );
}

#[tokio::test]
async fn a_date_with_its_cut_blocks_or_replaces_it_but_never_under_a_run() {
    let f = fixture().await;
    f.schedule(
        A,
        "'0 6,7 * * *', 'cut', 'clientes', 'block', NULL",
        six_am(),
    )
    .await;
    f.fire(A, six_am(), 24).await;
    let seven = six_am() + Duration::hours(1);
    let fired = f.fire(A, seven, 24).await;
    assert_eq!((fired[0].outcome, fired[0].job), (Outcome::Blocked, None));

    // 08:00 in Bogotá, the same day, asked to replace.
    let eight = six_am() + Duration::hours(2);
    let replace = "UPDATE quality.schedules SET rerun = 'replace',
                          next_fire_at = '2026-10-16 13:00:00+00' RETURNING id::text";
    f.as_tenant(A, replace).await;
    // Still extracting: not replaced under its run.
    assert_eq!(f.fire(A, eight, 24).await[0].outcome, Outcome::Blocked);
    f.as_tenant(
        A,
        "UPDATE quality.cuts SET state = 'done' RETURNING id::text",
    )
    .await;
    f.as_tenant(A, replace).await;
    let fired = f.fire(A, eight, 24).await;
    assert_eq!(fired[0].outcome, Outcome::Replaced);
    assert_eq!(
        f.as_tenant(
            A,
            "SELECT cut::text || ' run ' || run || ' ' || state FROM quality.cuts"
        )
        .await,
        ["2026-10-16 run 2 extracting"],
        "the same cut, not a second one"
    );
    let extractions = f
        .as_tenant(
            A,
            "SELECT id::text FROM quality.jobs WHERE kind = 'extract'",
        )
        .await;
    assert_eq!(extractions.len(), 2);
    // Each extraction carries its run beside the cut's id (0026).
    assert_eq!(
        f.as_tenant(
            A,
            "SELECT (parameters->>'run') || ' ' || (parameters->>'cut_id' = c.id::text)
               FROM quality.jobs j JOIN quality.cuts c ON j.kind = 'extract'
              ORDER BY 1"
        )
        .await,
        ["1 true", "2 true"]
    );
}

#[tokio::test]
async fn a_job_without_a_cut_is_queued_and_after_an_absence_runs_once() {
    let f = fixture().await;
    f.schedule(A, "'0 6 * * *', 'job', NULL, 'block', 'maintain'", six_am())
        .await;
    // Three mornings away.
    let now = six_am() + Duration::days(2) + Duration::hours(4);
    let fired = f.fire(A, now, 24).await;
    assert_eq!(
        (fired[0].outcome, fired[0].fire, fired[0].cut),
        (Outcome::Queued, Some(six_am() + Duration::days(2)), None)
    );
    assert_eq!(
        f.as_tenant(
            A,
            "SELECT kind || ' ' || coalesce(dataset, '-') FROM quality.jobs"
        )
        .await,
        ["maintain -"]
    );
    assert!(f
        .as_tenant(A, "SELECT id::text FROM quality.cuts")
        .await
        .is_empty());

    // Away longer than the window: nothing runs, the next fire moves on.
    f.as_tenant(
        A,
        "UPDATE quality.schedules SET next_fire_at = next_fire_at - interval '10 days' RETURNING id::text",
    )
    .await;
    let fired = f.fire(A, now, 2).await;
    assert_eq!((fired[0].outcome, fired[0].job), (Outcome::Skipped, None));
    assert_eq!(
        f.as_tenant(A, "SELECT id::text FROM quality.jobs")
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn only_the_vm_holding_the_lock_fires_and_another_takes_over() {
    let f = fixture().await;
    f.schedule(A, "'0 6 * * *', 'cut', 'clientes', 'block', NULL", six_am())
        .await;
    let mut one = Scheduler::new(f.app.clone(), 24);
    let mut two = Scheduler::new(f.app.clone(), 24);
    let fired = one.round(six_am()).await.unwrap();
    assert_eq!(fired.len(), 1);
    assert!(two.round(six_am()).await.is_none());

    // The first VM goes: its lock goes with its connection.
    drop(one);
    let tomorrow = six_am() + Duration::days(1);
    let mut taken = None;
    for _ in 0..50 {
        taken = two.round(tomorrow).await;
        if taken.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(taken.map(|f| f.len()), Some(1));
    assert_eq!(
        f.as_tenant(A, "SELECT cut::text FROM quality.cuts ORDER BY cut")
            .await,
        ["2026-10-16", "2026-10-17"]
    );
}
