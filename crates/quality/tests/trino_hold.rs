//! Holding the jobs that may use Trino while its coordinator restarts
//! (decision 0066, point 9), on an ephemeral PostgreSQL: the API holds a
//! tenant and counts the jobs running, the queue's role (as the workers
//! connect) takes no queued job that may use Trino while the hold is in
//! force and takes every other; released or expired, they go again.

use std::time::Duration;

use contracts::ModuleRegistry;
use infrastructure::module_migrations::PgModuleMigrator;
use quality::firing::{open_cut, CutRequest};
use quality::job_trino::subject;
use quality::module::QualityModule;
use quality::queue::{Queue, Timing};
use quality::schedule::Rerun;
use quality::source_tables;
use quality::trino_hold::{at_details, hold, release};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

const A: &str = "00000000-0000-0000-0000-00000000000a";
const B: &str = "00000000-0000-0000-0000-00000000000b";

struct Fixture {
    db: testkit::TestDb,
    /// The application's role, a tenant per task.
    app: PgPool,
    /// The queue's role.
    queue: PgPool,
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
        "ALTER TABLE quality.trino_holds OWNER TO app",
        "ALTER TABLE quality.sources OWNER TO app",
        "ALTER TABLE quality.cuts OWNER TO app",
    ] {
        sqlx::query(sql).execute(&db.pool).await.unwrap();
    }
    let app = infrastructure::db::with_tenancy(PgPoolOptions::new(), tenancy::Mode::Multiple)
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    let queue_options = (*db.pool.connect_options())
        .clone()
        .username("qd_queue")
        .password("queue");
    let queue = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(queue_options)
        .await
        .unwrap();
    Fixture { db, app, queue }
}

fn tenant(id: &str) -> tenancy::Tenant {
    tenancy::Tenant {
        id: id.into(),
        slug: String::new(),
    }
}

impl Fixture {
    /// A job of `kind` in `tenant`, in `state`, not known to use Trino or
    /// not.
    async fn job(&self, tenant: &str, kind: &str, state: &str) -> Uuid {
        self.marked(tenant, kind, state, None).await
    }

    /// A job of `kind` in `tenant`, in `state`, marked `uses` Trino.
    async fn marked(&self, tenant: &str, kind: &str, state: &str, uses: Option<bool>) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO quality.jobs (tenant_id, kind, dataset, state, worker, uses_trino)
             VALUES ($1::uuid, $2, 'clients', $3, CASE WHEN $3 = 'running' THEN 'w/1' END, $4)
             RETURNING id",
        )
        .bind(tenant)
        .bind(kind)
        .bind(state)
        .bind(uses)
        .fetch_one(&self.db.pool)
        .await
        .unwrap()
    }

    async fn hold(&self, of: &str, ttl: Duration) -> u64 {
        tenancy::within(tenant(of), hold(&self.app, "trino-1", ttl))
            .await
            .unwrap()
    }

    async fn release(&self, of: &str) -> bool {
        tenancy::within(tenant(of), release(&self.app))
            .await
            .unwrap()
    }

    /// The kind of the job a worker takes, offered `kinds`.
    async fn claim(&self, kinds: &[&str]) -> Option<String> {
        let queue = Queue::new(self.queue.clone(), "w/2", Timing::default());
        queue.claim(kinds).await.unwrap().map(|j| j.kind)
    }
}

const MINUTE: Duration = Duration::from_secs(60);

impl Fixture {
    /// The source `name` of `tenant`, on `driver`.
    async fn source(&self, tenant: &str, name: &str, driver: &str) {
        sqlx::query(
            "INSERT INTO quality.sources (tenant_id, name, driver, secrets)
             VALUES ($1::uuid, $2, $3, '')",
        )
        .bind(tenant)
        .bind(name)
        .bind(driver)
        .execute(&self.db.pool)
        .await
        .unwrap();
    }

    /// Whether the job is marked as using Trino.
    async fn uses(&self, job: Uuid) -> Option<bool> {
        sqlx::query_scalar("SELECT uses_trino FROM quality.jobs WHERE id = $1")
            .bind(job)
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    /// `dataset`'s cut, its mapping reading `from` if known: its extraction.
    async fn cut(&self, dataset: &str, from: Option<&str>) -> Uuid {
        let request = CutRequest {
            dataset,
            date: chrono::NaiveDate::from_ymd_opt(2026, 11, 11).unwrap(),
            rerun: Rerun::Block,
            schedule: None,
            requested_by: None,
            due_at: chrono::Utc::now(),
            from,
        };
        tenancy::within(tenant(A), async {
            let mut tx = self.app.begin().await.unwrap();
            let (_, _, job) = open_cut(&mut tx, &request).await.unwrap();
            tx.commit().await.unwrap();
            job.unwrap()
        })
        .await
    }

    /// The listing of `source`'s tables, queued.
    async fn listing(&self, source: &str) -> Uuid {
        tenancy::within(tenant(A), async {
            let mut tx = self.app.begin().await.unwrap();
            let job = source_tables::queue(&mut tx, source).await.unwrap();
            tx.commit().await.unwrap();
            job
        })
        .await
    }
}

#[tokio::test]
async fn a_job_is_marked_when_queued_by_its_sources_driver() {
    let f = fixture().await;
    f.source(A, "core", "sqlserver").await;
    f.source(A, "books", "postgres").await;
    assert_eq!(f.uses(f.listing("core").await).await, Some(true));
    assert_eq!(f.uses(f.listing("books").await).await, Some(false));
    assert_eq!(
        f.uses(f.listing("gone").await).await,
        None,
        "no such source"
    );
    // An extraction, by the source its mapping reads, when its enqueuer
    // read it; not known otherwise (the scheduler's), held while Trino is.
    assert_eq!(
        f.uses(f.cut("clients", Some("core")).await).await,
        Some(true)
    );
    assert_eq!(
        f.uses(f.cut("loans", Some("books")).await).await,
        Some(false)
    );
    assert_eq!(f.uses(f.cut("cards", None).await).await, None);
    // Another tenant's source of that name is not this one's.
    f.source(B, "other", "postgres").await;
    assert_eq!(f.uses(f.listing("other").await).await, None);
}

#[tokio::test]
async fn a_hold_counts_the_tenants_running_jobs_that_may_use_trino() {
    let f = fixture().await;
    f.job(A, "extract", "running").await;
    f.job(A, "tables", "running").await;
    f.job(A, "evaluate", "running").await;
    f.job(A, "records", "done").await;
    f.job(B, "map", "queued").await;
    assert_eq!(f.hold(A, MINUTE).await, 2);
    assert_eq!(f.hold(B, MINUTE).await, 0);
    // Asked again, the hold in force is extended and counted again.
    assert_eq!(f.hold(A, MINUTE).await, 2);
    assert!(f.release(A).await);
    assert!(!f.release(A).await, "released already");
    assert!(f.release(B).await);
}

#[tokio::test]
async fn while_held_a_job_that_may_use_trino_is_not_handed_out_and_others_are() {
    let f = fixture().await;
    f.job(A, "extract", "queued").await;
    f.job(A, "evaluate", "queued").await;
    f.hold(A, MINUTE).await;
    assert_eq!(f.claim(&["extract"]).await, None);
    assert_eq!(
        f.claim(&["extract", "evaluate"]).await.as_deref(),
        Some("evaluate")
    );
    // Another tenant's job is held only by its own tenant's hold.
    f.job(B, "map", "queued").await;
    assert_eq!(f.claim(&["map", "extract"]).await.as_deref(), Some("map"));
    assert!(f.release(A).await);
    assert_eq!(f.claim(&["extract"]).await.as_deref(), Some("extract"));
}

#[tokio::test]
async fn while_held_a_job_known_not_to_use_trino_is_handed_out() {
    let f = fixture().await;
    let on = f.marked(A, "extract", "queued", Some(true)).await;
    let unknown = f.marked(A, "extract", "queued", None).await;
    let off = f.marked(A, "extract", "queued", Some(false)).await;
    // A kind that never reads through Trino, whatever it is marked.
    f.marked(A, "evaluate", "queued", Some(true)).await;
    f.hold(A, MINUTE).await;
    let queue = Queue::new(f.queue.clone(), "w/2", Timing::default());
    let taken = queue.claim(&["extract"]).await.unwrap().map(|j| j.id);
    assert_eq!(taken, Some(off), "known not to use Trino: not held");
    assert_eq!(
        f.claim(&["extract"]).await,
        None,
        "on Trino or unknown: held"
    );
    assert_eq!(f.claim(&["evaluate"]).await.as_deref(), Some("evaluate"));
    assert!(f.release(A).await);
    let mut rest = Vec::new();
    while let Some(job) = queue.claim(&["extract"]).await.unwrap() {
        rest.push(job.id);
    }
    rest.sort();
    let mut held = vec![on, unknown];
    held.sort();
    assert_eq!(rest, held);
}

#[tokio::test]
async fn a_running_job_known_not_to_use_trino_is_not_counted() {
    let f = fixture().await;
    f.marked(A, "tables", "running", Some(false)).await;
    f.marked(A, "extract", "running", Some(false)).await;
    assert_eq!(f.hold(A, MINUTE).await, 0, "free: none uses Trino");
    f.marked(A, "map", "running", Some(true)).await;
    f.marked(A, "records", "running", None).await;
    f.marked(A, "evaluate", "running", Some(true)).await;
    assert_eq!(f.hold(A, MINUTE).await, 2, "on Trino, and in doubt");
}

#[tokio::test]
async fn a_hold_ends_by_itself_at_its_expiry() {
    let f = fixture().await;
    f.job(A, "records", "queued").await;
    f.hold(A, Duration::from_secs(1)).await;
    assert_eq!(f.claim(&["records"]).await, None);
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert_eq!(f.claim(&["records"]).await.as_deref(), Some("records"));
    assert!(!f.release(A).await, "it had expired: not in force");
}

#[tokio::test]
async fn a_claim_waits_for_a_hold_being_taken() {
    let f = fixture().await;
    f.job(A, "tables", "queued").await;
    // A hold's lock, taken and kept open as a hold's transaction does.
    let mut tx = f.db.pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(6600009)")
        .execute(&mut *tx)
        .await
        .unwrap();
    let queue = Queue::new(f.queue.clone(), "w/3", Timing::default());
    let claim = tokio::spawn(async move { queue.claim(&["tables"]).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!claim.is_finished(), "the claim waits for the hold's lock");
    tx.commit().await.unwrap();
    assert_eq!(
        claim.await.unwrap().map(|j| j.kind).as_deref(),
        Some("tables")
    );
}

#[tokio::test]
async fn the_kinds_held_are_those_whose_jobs_get_a_trino_token() {
    let f = fixture().await;
    let (t, j) = (Uuid::nil(), Uuid::nil());
    for kind in [
        "tables",
        "map",
        "records",
        "extract",
        "evaluate",
        "test_rule",
        "maintain",
        "lake_tables",
        "query",
    ] {
        let held: bool = sqlx::query_scalar("SELECT quality.may_use_trino($1)")
            .bind(kind)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
        let token = subject(t, j, kind, "core", Some("clients")).is_some();
        assert_eq!(held, token, "{kind}");
    }
}

#[tokio::test]
async fn a_held_job_goes_back_only_from_the_worker_that_asked_for_it() {
    let f = fixture().await;
    let job = f.marked(A, "extract", "running", Some(false)).await;
    sqlx::query("UPDATE quality.jobs SET attempts = 1, lease_until = now() + interval '1 minute'")
        .execute(&f.db.pool)
        .await
        .unwrap();
    f.hold(A, MINUTE).await;
    let state = || async {
        sqlx::query_as::<_, (String, Option<String>, i32)>(
            "SELECT state, worker, attempts FROM quality.jobs WHERE id = $1",
        )
        .bind(job)
        .fetch_one(&f.db.pool)
        .await
        .unwrap()
    };
    // Asked for by a worker that no longer holds it: it must not run, and
    // the run of the one that does (`w/1`) is left as it is.
    let other = tenancy::within(tenant(A), at_details(&f.app, job, "other", true)).await;
    assert!(!other.unwrap());
    assert_eq!(state().await, ("running".into(), Some("w/1".into()), 1));
    // Asked for by its own: back to the queue, its attempt given back.
    let own = tenancy::within(tenant(A), at_details(&f.app, job, "w", true)).await;
    assert!(!own.unwrap());
    assert_eq!(state().await, ("queued".into(), None, 0));
}
