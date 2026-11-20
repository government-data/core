//! Isolation between tenants with Row-Level Security.
//!
//! The tests connect as in production: a role without superuser or BYPASSRLS
//! that **owns** the tables (the owner skips RLS if the table has no FORCE).
//! The container's superuser only sets the scene. Needs Docker.
//!
//! The last tests cover the online edition's pool, which sets each task's
//! tenant on the connection it lends.

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
    let acme = testkit::new_tenant(&db, "acme").await;

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
    let acme = testkit::new_tenant(&db, "acme").await;

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

#[tokio::test]
async fn the_license_is_one_row_per_tenant() {
    let db = testkit::spawn_test_db().await;
    let app = pool_de_la_app(&db).await;
    let acme = testkit::new_tenant(&db, "acme").await;

    for tenant in [MAIN, acme.as_str()] {
        let mut tx = en(&app, tenant).await;
        for clave in ["ORQ-1", "ORQ-2"] {
            sqlx::query(
                "INSERT INTO orq_license (key_sealed) VALUES ($1)
                 ON CONFLICT (tenant_id) DO UPDATE SET key_sealed = $1",
            )
            .bind(format!("{clave}-{tenant}"))
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        let keys: Vec<String> = sqlx::query_scalar("SELECT key_sealed FROM orq_license")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert_eq!(keys, [format!("ORQ-2-{tenant}")]);
        tx.commit().await.unwrap();
    }
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

async fn pool_en_linea(db: &testkit::TestDb, conexiones: u32) -> PgPool {
    infrastructure::db::with_tenancy(
        PgPoolOptions::new().max_connections(conexiones),
        tenancy::Mode::Multiple,
    )
    .connect_with(testkit::app_role(db).await)
    .await
    .unwrap()
}

fn tenant(id: &str, slug: &str) -> tenancy::Tenant {
    tenancy::Tenant {
        id: id.into(),
        slug: slug.into(),
    }
}

async fn count_roles(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM iam_groups")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_online_pool_works_inside_the_tenant_of_each_task() {
    let db = testkit::spawn_test_db().await;
    let acme = testkit::new_tenant(&db, "acme").await;
    let pool = pool_en_linea(&db, 4).await;

    tenancy::within(tenant(&acme, "acme"), async {
        sqlx::query("INSERT INTO iam_groups (name) VALUES ('finance')")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(count_roles(&pool).await, 1);
    })
    .await;
    tenancy::within(tenant(MAIN, "main"), async {
        assert_eq!(count_roles(&pool).await, 0);
    })
    .await;

    // Without a tenant nothing is seen or written: it never falls into the main one.
    assert_eq!(count_roles(&pool).await, 0);
    let error = sqlx::query("INSERT INTO iam_groups (name) VALUES ('orphan')")
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(error.as_database_error().is_some(), "{error}");
}

/// Many tasks of two tenants over a pool of 2 connections: each connection is
/// reused with one tenant and the other, and nobody sees the other's rows.
#[tokio::test]
async fn reused_connections_never_carry_the_previous_tenant() {
    let db = testkit::spawn_test_db().await;
    let a = testkit::new_tenant(&db, "a").await;
    let b = testkit::new_tenant(&db, "b").await;
    let pool = pool_en_linea(&db, 2).await;

    let mut tasks = Vec::new();
    for i in 0..40 {
        let (id, slug) = if i % 2 == 0 { (&a, "a") } else { (&b, "b") };
        let pool = pool.clone();
        let tenant = tenant(id, slug);
        tasks.push(tokio::spawn(tenancy::within(tenant, async move {
            sqlx::query("INSERT INTO iam_groups (name) VALUES ($1)")
                .bind(format!("role-{i}"))
                .execute(&pool)
                .await
                .unwrap();
            let names: Vec<String> = sqlx::query_scalar("SELECT name FROM iam_groups")
                .fetch_all(&pool)
                .await
                .unwrap();
            // Only its own: all with the same parity as its own.
            assert!(
                names.iter().all(|n| {
                    let j: usize = n.trim_start_matches("role-").parse().unwrap();
                    j % 2 == i % 2
                }),
                "task {i} saw {names:?}"
            );
        })));
    }
    for task in tasks {
        task.await.unwrap();
    }
    for (id, slug) in [(&a, "a"), (&b, "b")] {
        let total = tenancy::within(tenant(id, slug), count_roles(&pool)).await;
        assert_eq!(total, 20);
    }
}

#[tokio::test]
async fn the_directory_finds_tenants_by_subdomain_and_cell() {
    use tenancy::Directory;
    let db = testkit::spawn_test_db().await;
    testkit::new_tenant(&db, "acme").await;
    sqlx::query("INSERT INTO tenants (slug, name, status) VALUES ('old', 'Old', 'closed')")
        .execute(&db.pool)
        .await
        .unwrap();
    let directory = infrastructure::PgTenantDirectory::new(db.pool.clone());

    let acme = directory.by_slug("acme").await.unwrap().unwrap();
    assert_eq!(acme.status, tenancy::Status::Active);
    assert_eq!(acme.cell, "main");
    assert!(directory.by_slug("nadie").await.unwrap().is_none());
    let slugs: Vec<String> = directory
        .in_cell("main")
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.tenant.slug)
        .collect();
    assert_eq!(slugs, ["acme", "main"]);
}
