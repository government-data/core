//! The audit chain's trigger, guards and adoption (decision 0070, step 1),
//! on an ephemeral PostgreSQL. The trigger's hashes are recomputed by the
//! Rust encoder (`domain::audit_chain`), which is what the verifier will use.
//! Synthetic rows only. Needs Docker.

use std::borrow::Cow;
use std::time::{Duration, Instant, SystemTime};

use domain::audit_chain::{genesis, row_hash, ChainHash, ChainRow, CHAIN_COLUMNS, FIELDS};
use domain::{
    AdminId, AuditEventType, AuditRepository, NewActionAudit, NewAuditEvent, NewViewAudit,
    ViewShown,
};
use infrastructure::PgAuditRepository;
use sqlx::postgres::{PgConnectOptions, PgPool};
use sqlx::Row;

const TENANT: &str = "00000000-0000-0000-0000-000000000001";
const OTHER: &str = "00000000-0000-0000-0000-0000000000b2";

/// Every column the chain reads, as the verifier will read it.
const READ: &str = "SELECT chain_seq, chain_hash, chain_version, tenant_id::text AS tenant_id, \
     id::text AS id, event_type, admin_id::text AS admin_id, email_attempted, ip, user_agent, \
     (extract(epoch FROM occurred_at) * 1000000)::bigint AS micros, action, resource, \
     service_account, job_id::text AS job_id, purpose, reference, rows_shown::bigint AS rows, \
     fields_shown FROM admin_audit_events WHERE tenant_id = $1::uuid ORDER BY chain_seq";

/// A tenant's rows in sequence order: each one's number, stored hash and fields.
async fn chain(pool: &PgPool, tenant: &str) -> Vec<(i64, Vec<u8>, ChainRow)> {
    let rows = sqlx::query(READ)
        .bind(tenant)
        .fetch_all(pool)
        .await
        .unwrap();
    rows.iter()
        .map(|r| {
            assert_eq!(r.get::<i16, _>("chain_version"), 1);
            let row = ChainRow {
                tenant_id: r.get("tenant_id"),
                id: r.get("id"),
                event_type: r.get("event_type"),
                admin_id: r.get("admin_id"),
                email_attempted: r.get("email_attempted"),
                ip: r.get("ip"),
                user_agent: r.get("user_agent"),
                occurred_at_micros: r.get("micros"),
                action: r.get("action"),
                resource: r.get("resource"),
                service_account: r.get("service_account"),
                job_id: r.get("job_id"),
                purpose: r.get("purpose"),
                reference: r.get("reference"),
                rows_shown: r.get("rows"),
                fields_shown: r.get("fields_shown"),
            };
            (r.get("chain_seq"), r.get("chain_hash"), row)
        })
        .collect()
}

/// Recomputes a tenant's chain in Rust: numbers dense from 1, every hash
/// equal to the trigger's, the head on the last row. Returns the row count.
async fn verify(pool: &PgPool, tenant: &str) -> i64 {
    let mut prev: ChainHash = genesis(tenant);
    let rows = chain(pool, tenant).await;
    for (n, (seq, stored, row)) in (1..).zip(&rows) {
        assert_eq!(*seq, n, "the numbers are dense");
        let hash = row_hash(&prev, n, row);
        assert_eq!(
            stored.as_slice(),
            hash,
            "row {n}'s hash, Rust against the trigger"
        );
        prev = hash;
    }
    let head: (i64, Vec<u8>) =
        sqlx::query_as("SELECT seq, hash FROM audit_chain_heads WHERE tenant_id = $1::uuid")
            .bind(tenant)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(head, (rows.len() as i64, prev.to_vec()), "the head");
    rows.len() as i64
}

/// A row written as SQL, with what the repository never writes: microseconds,
/// a NULL array element, letters outside ASCII.
async fn insert(pool: &PgPool, tenant: &str, micros: i64, fields: Option<Vec<Option<&str>>>) {
    sqlx::query(
        "INSERT INTO admin_audit_events (tenant_id, event_type, ip, occurred_at, email_attempted, \
         user_agent, action, resource, purpose, reference, rows_shown, fields_shown) \
         VALUES ($1::uuid, 'records_viewed', '198.51.100.7', \
         timestamptz 'epoch' + $2 * interval '1 microsecond', 'ñandú@example.com', 'agente/1 ✓', \
         'quality:ViewPersonalData', 'dataset/socios', 'quality_review', 'TCK-ñ', 0, $3)",
    )
    .bind(tenant)
    .bind(micros)
    .bind(fields)
    .execute(pool)
    .await
    .unwrap();
}

/// The second tenant, beside the installation's.
async fn other_tenant(pool: &PgPool) {
    sqlx::query("INSERT INTO tenants (id, slug, name) VALUES ($1::uuid, 'otra', 'Otra')")
        .bind(OTHER)
        .execute(pool)
        .await
        .unwrap();
}

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

fn action(event_type: AuditEventType, secs: u64) -> NewActionAudit {
    NewActionAudit {
        event_type,
        admin_id: Some(AdminId::new("00000000-0000-0000-0000-0000000000a1")),
        service_account: Some("qd-worker-1".into()),
        job_id: Some("00000000-0000-0000-0000-0000000000c3".into()),
        action: "job:extract".into(),
        resource: "source/core".into(),
        ip: "203.0.113.9".into(),
        user_agent: None,
        occurred_at: at(secs),
    }
}

/// Each way the product writes a row: a sign-in, an action, a view.
async fn write_each_kind(repo: &PgAuditRepository, secs: u64) {
    repo.record(&NewAuditEvent {
        event_type: AuditEventType::LoginFailed,
        admin_id: None,
        email_attempted: Some("someone@example.com".into()),
        ip: "203.0.113.1".into(),
        user_agent: Some("curl/8.0".into()),
        occurred_at: at(secs),
    })
    .await
    .unwrap();
    repo.record_action(&action(AuditEventType::CredentialsIssued, secs))
        .await
        .unwrap();
    repo.record_view(&NewViewAudit {
        action: action(AuditEventType::RecordsViewed, secs),
        shown: ViewShown {
            purpose: "quality_review".into(),
            reference: None,
            rows_shown: 25,
            fields_shown: vec!["nombre".into(), "año".into()],
        },
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn the_rust_encoder_equals_the_trigger() {
    let db = testkit::spawn_test_db().await;
    let repo = PgAuditRepository::new(db.pool.clone());
    write_each_kind(&repo, 1_800_000_000).await;
    write_each_kind(&repo, 1_800_000_000).await; // the same second
    insert(&db.pool, TENANT, 1_800_000_000_123_456, None).await;
    insert(&db.pool, TENANT, -1, Some(vec![Some("ñ"), None, Some("")])).await;
    insert(&db.pool, TENANT, 0, Some(vec![])).await;
    other_tenant(&db.pool).await;
    insert(&db.pool, OTHER, 1_800_000_001_000_001, Some(vec![None])).await;
    insert(&db.pool, OTHER, 1_800_000_001_000_001, None).await;

    assert_eq!(verify(&db.pool, TENANT).await, 9);
    assert_eq!(verify(&db.pool, OTHER).await, 2, "a chain of its own");
}

#[tokio::test]
async fn every_audit_column_is_hashed_or_the_chains_own() {
    let db = testkit::spawn_test_db().await;
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name::text FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name = 'admin_audit_events'",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    for column in &columns {
        assert!(
            FIELDS.contains(&column.as_str()) || CHAIN_COLUMNS.contains(&column.as_str()),
            "`{column}` is neither hashed (domain::audit_chain::FIELDS and audit_row_v1) \
             nor one of the chain's own columns"
        );
    }
    assert_eq!(columns.len(), FIELDS.len() + CHAIN_COLUMNS.len());
}

#[tokio::test]
async fn eight_writers_at_once_leave_a_dense_verifying_chain() {
    let db = testkit::spawn_test_db().await;
    let writers = (0..8u64).map(|w| {
        let repo = PgAuditRepository::new(db.pool.clone());
        tokio::spawn(async move {
            for i in 0..25 {
                write_each_kind(&repo, 1_800_000_000 + w * 100 + i).await;
            }
        })
    });
    for writer in writers.collect::<Vec<_>>() {
        writer.await.unwrap();
    }
    assert_eq!(verify(&db.pool, TENANT).await, 8 * 25 * 3);
}

#[tokio::test]
async fn update_delete_and_truncate_are_refused() {
    let db = testkit::spawn_test_db().await;
    insert(&db.pool, TENANT, 0, None).await;
    for sql in [
        "UPDATE admin_audit_events SET ip = '192.0.2.1'",
        "DELETE FROM admin_audit_events",
        "DELETE FROM admin_audit_events WHERE false",
        "TRUNCATE admin_audit_events",
    ] {
        let error = sqlx::query(sql).execute(&db.pool).await.unwrap_err();
        assert!(error.to_string().contains("append-only"), "{sql}: {error}");
    }
    assert_eq!(verify(&db.pool, TENANT).await, 1);
}

#[tokio::test]
async fn a_new_tenant_opens_its_chain_and_one_with_rows_but_no_head_is_refused() {
    let db = testkit::spawn_test_db().await;
    other_tenant(&db.pool).await;
    insert(&db.pool, OTHER, 0, None).await;
    let opened: (i64, i64) =
        sqlx::query_as("SELECT seq, adopted_seq FROM audit_chain_heads WHERE tenant_id = $1::uuid")
            .bind(OTHER)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(opened, (1, 0), "opened at the genesis, nothing adopted");

    sqlx::query("DELETE FROM audit_chain_heads")
        .execute(&db.pool)
        .await
        .unwrap();
    let error = sqlx::query(
        "INSERT INTO admin_audit_events (tenant_id, event_type, ip, occurred_at) \
         VALUES ($1::uuid, 'login_failed', '203.0.113.1', now())",
    )
    .bind(OTHER)
    .execute(&db.pool)
    .await
    .unwrap_err();
    assert!(error.to_string().contains("no head"), "{error}");
}

#[tokio::test]
async fn a_writer_waits_for_the_head_five_seconds_at_most() {
    let db = testkit::spawn_test_db().await;
    insert(&db.pool, TENANT, 0, None).await;
    let mut holder = db.pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM audit_chain_heads FOR UPDATE")
        .execute(&mut *holder)
        .await
        .unwrap();

    let started = Instant::now();
    let repo = PgAuditRepository::new(db.pool.clone());
    let failed = repo
        .record_action(&action(AuditEventType::RunStarted, 1))
        .await;
    let waited = started.elapsed();
    assert!(failed.unwrap_err().to_string().contains("lock timeout"));
    assert!(
        waited >= Duration::from_secs(5) && waited < Duration::from_secs(10),
        "{waited:?}"
    );
    holder.rollback().await.unwrap();
    assert_eq!(verify(&db.pool, TENANT).await, 1, "nothing half-written");
}

/// A database owned by a role as the installer makes it (no superuser, no
/// BYPASSRLS), migrated up to the one before the chain.
async fn before_the_chain(db: &testkit::TestDb) -> (PgConnectOptions, PgConnectOptions) {
    for sql in [
        "CREATE ROLE chain_owner LOGIN PASSWORD 'owner' NOSUPERUSER NOBYPASSRLS",
        "CREATE DATABASE adopt OWNER chain_owner",
    ] {
        sqlx::query(sql).execute(&db.pool).await.expect(sql);
    }
    let superuser = db.pool.connect_options().as_ref().clone().database("adopt");
    let su = PgPool::connect_with(superuser.clone()).await.unwrap();
    sqlx::query("CREATE EXTENSION IF NOT EXISTS pgcrypto")
        .execute(&su)
        .await
        .unwrap();
    let owner = superuser.clone().username("chain_owner").password("owner");
    let pool = PgPool::connect_with(owner.clone()).await.unwrap();
    let earlier = sqlx::migrate::Migrator {
        migrations: Cow::Owned(
            infrastructure::MIGRATOR
                .iter()
                .take_while(|m| m.description != "audit chain")
                .cloned()
                .collect(),
        ),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    earlier.run(&pool).await.unwrap();
    other_tenant(&su).await;
    (superuser, owner)
}

/// A row as the previous version writes it, under its tenant's isolation.
async fn earlier_row(pool: &PgPool, tenant: &str, secs: i64, ip: &str) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(tenant)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO admin_audit_events (event_type, ip, occurred_at, email_attempted) \
         VALUES ('login_failed', $1, to_timestamp($2), 'ñ@example.com')",
    )
    .bind(ip)
    .bind(secs)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn existing_rows_of_every_tenant_are_adopted_by_replicas_starting_together() {
    let db = testkit::spawn_test_db().await;
    let (superuser, owner) = before_the_chain(&db).await;
    let pool = PgPool::connect_with(owner.clone()).await.unwrap();
    for i in 0..30 {
        earlier_row(&pool, TENANT, 1_800_000_000 + i / 4, "203.0.113.1").await;
    }
    earlier_row(&pool, OTHER, 1_800_000_000, "203.0.113.2").await;

    let second = PgPool::connect_with(owner).await.unwrap();
    let (a, b) = tokio::join!(
        infrastructure::run_migrations(&pool),
        infrastructure::run_migrations(&second)
    );
    a.unwrap();
    b.unwrap();

    let su = PgPool::connect_with(superuser).await.unwrap();
    assert_eq!(verify(&su, TENANT).await, 30);
    assert_eq!(verify(&su, OTHER).await, 1);
    let order: Vec<bool> = sqlx::query_scalar(
        "SELECT occurred_at >= lag(occurred_at, 1, occurred_at) OVER (ORDER BY chain_seq) \
         FROM admin_audit_events WHERE tenant_id = $1::uuid",
    )
    .bind(TENANT)
    .fetch_all(&su)
    .await
    .unwrap();
    assert!(
        order.iter().all(|in_order| *in_order),
        "adopted by occurred_at"
    );
    let adopted: Vec<i64> =
        sqlx::query_scalar("SELECT adopted_seq FROM audit_chain_heads ORDER BY 1")
            .fetch_all(&su)
            .await
            .unwrap();
    assert_eq!(adopted, vec![1, 30]);
    let forced: Vec<bool> = sqlx::query_scalar(
        "SELECT relforcerowsecurity FROM pg_class \
         WHERE relname IN ('admin_audit_events', 'audit_chain_heads')",
    )
    .fetch_all(&su)
    .await
    .unwrap();
    assert_eq!(forced, vec![true, true], "row-level security forced again");

    earlier_row(&pool, TENANT, 1_800_000_100, "203.0.113.3").await;
    assert_eq!(
        verify(&su, TENANT).await,
        31,
        "the chain goes on after adoption"
    );
}
