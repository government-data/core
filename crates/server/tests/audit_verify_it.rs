//! `server audit verify`'s verifier over PostgreSQL (decision 0070, step 2):
//! the application's `verify_all` on `infrastructure::PgAuditChain`, as the
//! table's owner under forced row-level security, as in production.
//! Synthetic rows only. Needs Docker.

use std::borrow::Cow;

use application::audit_chain::verify_all;
use domain::audit_chain_check::{BreakKind, ChainBreak, ChainReport};
use infrastructure::PgAuditChain;
use sqlx::postgres::PgPool;

/// The main tenant, the installation's one chain (decision 0076).
const T1: &str = "00000000-0000-0000-0000-000000000001";

/// The owner's pool and the superuser's, on a database migrated up to the
/// one before the chain, with rows written then, then migrated whole
/// (adoption), then rows written after.
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
    write(&owner, T1, 0..5).await;
    infrastructure::run_migrations(&owner).await.unwrap();
    write(&owner, T1, 5..9).await;
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
async fn adopted_and_new_rows_verify_and_each_run_is_kept() {
    let db = testkit::spawn_test_db().await;
    let (owner, su) = adopted(&db).await;
    let reports = verify(&owner, None).await;
    assert_eq!(reports.len(), 1, "one chain");
    let r = of(&reports, T1);
    assert_eq!(r.first_break, None);
    assert_eq!(
        (r.rows_checked, r.first_seq, r.last_seq),
        (9, Some(1), Some(9))
    );
    assert_eq!(r.adopted_seq, Some(5));
    assert_eq!(r.head.as_ref().map(|(seq, _)| *seq), Some(9));
    let kept: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT tenant_id::text, rows_checked, result FROM audit_chain_checks ORDER BY tenant_id",
    )
    .fetch_all(&su)
    .await
    .unwrap();
    assert_eq!(kept, vec![(T1.into(), 9, "ok".into())]);
}

#[tokio::test]
async fn a_row_changed_by_the_owner_is_a_hash_break() {
    let db = testkit::spawn_test_db().await;
    let (owner, su) = adopted(&db).await;
    unguarded(
        &owner,
        T1,
        "UPDATE admin_audit_events SET ip = '198.51.100.1' WHERE chain_seq = $1",
        7,
    )
    .await;

    let reports = verify(&owner, None).await;
    assert_eq!(of(&reports, T1).first_break, at(7, BreakKind::Hash));
    assert_eq!(of(&reports, T1).rows_checked, 6, "up to the break");
    let kept: Vec<(String, Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT result, break_seq, break_kind FROM audit_chain_checks \
         WHERE tenant_id = $1::uuid",
    )
    .bind(T1)
    .fetch_all(&su)
    .await
    .unwrap();
    assert_eq!(kept, vec![("break".into(), Some(7), Some("hash".into()))]);
    let only = verify(&owner, Some(T1)).await;
    assert_eq!(only.len(), 1);
    assert_eq!(only[0].first_break, at(7, BreakKind::Hash));
}

#[tokio::test]
async fn a_row_removed_by_the_owner_is_a_gap() {
    let db = testkit::spawn_test_db().await;
    let (owner, _su) = adopted(&db).await;
    unguarded(
        &owner,
        T1,
        "DELETE FROM admin_audit_events WHERE chain_seq = $1",
        2,
    )
    .await;
    let reports = verify(&owner, None).await;
    assert_eq!(of(&reports, T1).first_break, at(2, BreakKind::Gap));
}
