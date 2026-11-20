//! `server audit verify`'s verifier over PostgreSQL (decision 0070, step 2):
//! the application's `verify_all` on `infrastructure::PgAuditChain`, as the
//! table's owner under forced row-level security, as in production.
//! Synthetic rows only. Needs Docker.

use std::borrow::Cow;

use application::audit_chain::verify_all;
use domain::audit_chain_check::{BreakKind, ChainBreak, ChainReport};
use infrastructure::PgAuditChain;
use sqlx::postgres::PgPool;

const T1: &str = "00000000-0000-0000-0000-000000000001";
const T2: &str = "00000000-0000-0000-0000-0000000000b2";

/// The owner's pool and the superuser's, on a database migrated up to the
/// one before the chain, with rows of two tenants written then, then
/// migrated whole (adoption), then rows written after.
async fn adopted(db: &testkit::TestDb) -> (PgPool, PgPool) {
    for sql in [
        "CREATE ROLE chain_owner LOGIN PASSWORD 'owner' NOSUPERUSER NOBYPASSRLS",
        "CREATE DATABASE verify OWNER chain_owner",
    ] {
        sqlx::query(sql).execute(&db.pool).await.expect(sql);
    }
    let superuser = db
        .pool
        .connect_options()
        .as_ref()
        .clone()
        .database("verify");
    let su = PgPool::connect_with(superuser.clone()).await.unwrap();
    sqlx::query("CREATE EXTENSION IF NOT EXISTS pgcrypto")
        .execute(&su)
        .await
        .unwrap();
    let owner = PgPool::connect_with(superuser.username("chain_owner").password("owner"))
        .await
        .unwrap();
    let before_the_chain = sqlx::migrate::Migrator {
        migrations: Cow::Owned(
            infrastructure::MIGRATOR
                .iter()
                .take_while(|m| m.description != "audit chain")
                .cloned()
                .collect(),
        ),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    before_the_chain.run(&owner).await.unwrap();
    sqlx::query("INSERT INTO tenants (id, slug, name) VALUES ($1::uuid, 'otra', 'Otra')")
        .bind(T2)
        .execute(&su)
        .await
        .unwrap();
    write(&owner, T1, 0..5).await;
    write(&owner, T2, 0..3).await;
    infrastructure::run_migrations(&owner).await.unwrap();
    write(&owner, T1, 5..9).await;
    write(&owner, T2, 3..4).await;
    (owner, su)
}

/// Rows `n` under `tenant`'s isolation, as the application writes them.
async fn write(pool: &PgPool, tenant: &str, n: std::ops::Range<i64>) {
    for i in n {
        as_tenant(
            pool,
            tenant,
            "INSERT INTO admin_audit_events (event_type, ip, occurred_at, action) \
             VALUES ('credentials_issued', '203.0.113.9', to_timestamp(1800000000 + $1), \
             'job:extract')",
            i,
        )
        .await;
    }
}

/// One statement with one bound number, in a transaction under `tenant`.
async fn as_tenant(pool: &PgPool, tenant: &str, sql: &str, n: i64) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(tenant)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(sql).bind(n).execute(&mut *tx).await.expect(sql);
    tx.commit().await.unwrap();
}

/// What the owner does with the guard disabled, as the research did.
async fn unguarded(owner: &PgPool, tenant: &str, sql: &str, n: i64) {
    let guard = |on: bool| {
        format!(
            "ALTER TABLE admin_audit_events {} TRIGGER audit_chain_guard",
            if on { "ENABLE" } else { "DISABLE" }
        )
    };
    sqlx::query(&guard(false)).execute(owner).await.unwrap();
    as_tenant(owner, tenant, sql, n).await;
    sqlx::query(&guard(true)).execute(owner).await.unwrap();
}

async fn verify(owner: &PgPool, only: Option<&str>) -> Vec<ChainReport> {
    let chain = PgAuditChain::new(owner.clone());
    verify_all(&chain, &chain, only).await.unwrap()
}

fn of<'a>(reports: &'a [ChainReport], tenant: &str) -> &'a ChainReport {
    reports.iter().find(|r| r.tenant == tenant).unwrap()
}

fn at(seq: i64, kind: BreakKind) -> Option<ChainBreak> {
    Some(ChainBreak { seq, kind })
}

#[tokio::test]
async fn adopted_and_new_rows_of_two_tenants_verify_and_each_run_is_kept() {
    let db = testkit::spawn_test_db().await;
    let (owner, su) = adopted(&db).await;
    let reports = verify(&owner, None).await;
    for (tenant, rows, adopted) in [(T1, 9, 5), (T2, 4, 3)] {
        let r = of(&reports, tenant);
        assert_eq!(r.first_break, None, "{tenant}");
        assert_eq!(
            (r.rows_checked, r.first_seq, r.last_seq),
            (rows, Some(1), Some(rows))
        );
        assert_eq!(r.adopted_seq, Some(adopted));
        assert_eq!(r.head.as_ref().map(|(seq, _)| *seq), Some(rows));
    }
    let kept: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT tenant_id::text, rows_checked, result FROM audit_chain_checks ORDER BY tenant_id",
    )
    .fetch_all(&su)
    .await
    .unwrap();
    assert_eq!(
        kept,
        vec![(T1.into(), 9, "ok".into()), (T2.into(), 4, "ok".into())]
    );
}

#[tokio::test]
async fn a_row_changed_by_the_owner_is_a_hash_break_and_a_removed_one_a_gap() {
    let db = testkit::spawn_test_db().await;
    let (owner, su) = adopted(&db).await;
    unguarded(
        &owner,
        T1,
        "UPDATE admin_audit_events SET ip = '198.51.100.1' WHERE chain_seq = $1",
        7,
    )
    .await;
    unguarded(
        &owner,
        T2,
        "DELETE FROM admin_audit_events WHERE chain_seq = $1",
        2,
    )
    .await;

    let reports = verify(&owner, None).await;
    assert_eq!(of(&reports, T1).first_break, at(7, BreakKind::Hash));
    assert_eq!(of(&reports, T1).rows_checked, 6, "up to the break");
    assert_eq!(of(&reports, T2).first_break, at(2, BreakKind::Gap));
    let only = verify(&owner, Some(T2)).await;
    assert_eq!(only.len(), 1);
    assert_eq!(only[0].first_break, at(2, BreakKind::Gap));

    let kept: Vec<(String, Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT result, break_seq, break_kind FROM audit_chain_checks \
         WHERE tenant_id = $1::uuid",
    )
    .bind(T1)
    .fetch_all(&su)
    .await
    .unwrap();
    assert_eq!(kept, vec![("break".into(), Some(7), Some("hash".into()))]);
}
