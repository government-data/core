//! The queue on an ephemeral PostgreSQL, as the workers use it: connected
//! as the queue's role (not a superuser, so row security applies), two
//! workers drain it without taking a job twice, and a job whose worker
//! stopped renewing is taken again (decision 0018's "done when").

use std::collections::HashSet;
use std::time::Duration;

use contracts::ModuleRegistry;
use infrastructure::module_migrations::PgModuleMigrator;
use quality::module::QualityModule;
use quality::queue::{Queue, Timing};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

const TENANTS: [&str; 2] = [
    "00000000-0000-0000-0000-00000000000a",
    "00000000-0000-0000-0000-00000000000b",
];

/// A migrated database with `jobs` jobs per tenant, and a pool connected as
/// the queue's role.
async fn queue_db(jobs: usize) -> (testkit::TestDb, PgPool) {
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
    for tenant in TENANTS {
        for i in 0..jobs {
            sqlx::query("INSERT INTO quality.jobs (tenant_id, kind, dataset) VALUES ($1::uuid, 'evaluate', $2)")
                .bind(tenant)
                .bind(format!("dataset-{i}"))
                .execute(&db.pool)
                .await
                .unwrap();
        }
    }
    let options = (*db.pool.connect_options())
        .clone()
        .username("qd_queue")
        .password("queue");
    let queue = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    (db, queue)
}

#[tokio::test]
async fn two_workers_drain_the_queue_without_taking_a_job_twice() {
    let (db, pool) = queue_db(10).await;
    let work = |name: &'static str| {
        let queue = Queue::new(pool.clone(), name, Timing::default());
        tokio::spawn(async move {
            let mut taken = Vec::new();
            while let Some(job) = queue.claim(&["evaluate"]).await.unwrap() {
                assert!(queue.finish(&job, None).await.unwrap());
                taken.push((job.tenant_id, job.id));
            }
            taken
        })
    };
    let (a, b) = (work("worker-a"), work("worker-b"));
    let taken: Vec<_> = a
        .await
        .unwrap()
        .into_iter()
        .chain(b.await.unwrap())
        .collect();
    let unique: HashSet<_> = taken.iter().collect();
    assert_eq!((taken.len(), unique.len()), (20, 20));
    let tenants: HashSet<_> = taken.iter().map(|(t, _)| t.to_string()).collect();
    assert_eq!(
        tenants.len(),
        2,
        "the queue's role takes every tenant's jobs"
    );
    let done: i64 = sqlx::query_scalar("SELECT count(*) FROM quality.jobs WHERE state = 'done'")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(done, 20);
    assert!(Queue::new(pool, "worker-c", Timing::default())
        .claim(&["map"])
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn a_job_whose_lease_ran_out_is_taken_again_until_its_attempts_end() {
    let (db, pool) = queue_db(1).await;
    sqlx::query("DELETE FROM quality.jobs WHERE tenant_id = $1::uuid")
        .bind(TENANTS[1])
        .execute(&db.pool)
        .await
        .unwrap();
    let timing = Timing {
        lease: Duration::from_millis(300),
        backoff: Duration::ZERO,
    };
    let (stalled, other) = (
        Queue::new(pool.clone(), "stalled", timing),
        Queue::new(pool, "other", timing),
    );

    let first = stalled.claim(&["evaluate"]).await.unwrap().unwrap();
    assert_eq!(first.attempts, 1);
    assert!(
        other.claim(&["evaluate"]).await.unwrap().is_none(),
        "held while its lease lasts"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;

    let second = other.claim(&["evaluate"]).await.unwrap().unwrap();
    assert_eq!((second.id, second.attempts), (first.id, 2));
    assert!(
        !stalled.renew(&first).await.unwrap(),
        "the stalled worker lost it"
    );
    assert!(!stalled.finish(&first, None).await.unwrap());

    assert!(other
        .finish(&second, Some("source timed out"))
        .await
        .unwrap());
    let third = other.claim(&["evaluate"]).await.unwrap().unwrap();
    assert_eq!(third.attempts, 3);
    assert!(other
        .finish(&third, Some("source timed out"))
        .await
        .unwrap());
    assert!(
        other.claim(&["evaluate"]).await.unwrap().is_none(),
        "no attempts left"
    );
    let (state, error): (String, String) =
        sqlx::query_as("SELECT state, error FROM quality.jobs WHERE id = $1")
            .bind(first.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        (state.as_str(), error.as_str()),
        ("failed", "source timed out")
    );
}
