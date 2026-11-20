//! The queue's ready jobs by kind, on an ephemeral PostgreSQL as the
//! application's role with row security per tenant (decision 0071, point
//! 8): only jobs queued and past their backoff count, each tenant sees its
//! own, and the oldest's wait is from when it became ready.

use contracts::ModuleRegistry;
use infrastructure::module_migrations::PgModuleMigrator;
use quality::module::QualityModule;
use quality::queue_ready::{ready, Ready};
use sqlx::postgres::PgPoolOptions;

const A: &str = "00000000-0000-0000-0000-00000000000a";
const B: &str = "00000000-0000-0000-0000-00000000000b";

fn tenant(id: &str) -> tenancy::Tenant {
    tenancy::Tenant {
        id: id.into(),
        slug: String::new(),
    }
}

#[tokio::test]
async fn ready_jobs_are_counted_by_kind_within_the_tenant() {
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
    ] {
        sqlx::query(sql).execute(&db.pool).await.unwrap();
    }
    // Synthetic jobs: A has two evaluations ready (one waiting ten minutes),
    // one in backoff, one running and one done; B has one map ready.
    for (t, kind, state, not_before) in [
        (A, "evaluate", "queued", "now() - interval '10 minutes'"),
        (A, "evaluate", "queued", "now()"),
        (A, "evaluate", "queued", "now() + interval '5 minutes'"),
        (A, "extract", "running", "now()"),
        (A, "map", "done", "now()"),
        (B, "map", "queued", "now()"),
    ] {
        sqlx::query(&format!(
            "INSERT INTO quality.jobs (tenant_id, kind, dataset, state, not_before)
             VALUES ($1::uuid, $2, 'dataset-x', $3, {not_before})"
        ))
        .bind(t)
        .bind(kind)
        .bind(state)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    let app = infrastructure::db::with_tenancy(PgPoolOptions::new(), tenancy::Mode::Multiple)
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap();

    let a: Vec<Ready> = tenancy::within(tenant(A), ready(&app)).await.unwrap();
    assert_eq!(a.len(), 1, "{a:?}");
    assert_eq!((a[0].kind.as_str(), a[0].jobs), ("evaluate", 2));
    assert!((590.0..700.0).contains(&a[0].oldest_seconds), "{a:?}");

    let b = tenancy::within(tenant(B), ready(&app)).await.unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!((b[0].kind.as_str(), b[0].jobs), ("map", 1));
}
