//! Postgres adapter for the audit chain's verifier (decision 0070, point
//! 11): the chain's sealed rows in `chain_seq` order, a page at a time by the
//! `(tenant_id, chain_seq)` index, its head, and each run kept in
//! `audit_chain_checks`. One installation is one organisation (decision
//! 0076): there is one chain, the main tenant's, which row-level security
//! leaves the application's role without a session setting.

use async_trait::async_trait;
use domain::audit_chain::ChainRow;
use domain::audit_chain_check::{
    AuditChainChecks, AuditChainRows, ChainHead, ChainReport, ChainTail, SealedRow,
};
use domain::RepositoryError;
use sqlx::postgres::{PgPool, PgRow, Postgres};
use sqlx::{Row, Transaction};

/// The main tenant, the only one (decision 0076, point 6).
const MAIN: &str = "00000000-0000-0000-0000-000000000001";

fn backend(err: sqlx::Error) -> RepositoryError {
    RepositoryError::Backend(err.to_string())
}

/// Every column the chain hashes, as the encoding reads it (uuids as text,
/// the time as microseconds).
const ROWS: &str = "SELECT chain_seq, chain_hash, chain_version, tenant_id::text AS tenant_id, \
     id::text AS id, event_type, admin_id::text AS admin_id, email_attempted, ip, user_agent, \
     (extract(epoch FROM occurred_at) * 1000000)::bigint AS micros, action, resource, \
     service_account, job_id::text AS job_id, purpose, reference, rows_shown::bigint AS rows, \
     fields_shown FROM admin_audit_events \
     WHERE tenant_id = $1::uuid AND chain_seq > $2 AND chain_seq <= $3 \
     ORDER BY chain_seq LIMIT $4";

/// The audit chain in PostgreSQL, as the verifier reads it.
#[derive(Debug, Clone)]
pub struct PgAuditChain {
    pool: PgPool,
}

impl PgAuditChain {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// A transaction: each read sees one snapshot.
    async fn begin(&self) -> Result<Transaction<'static, Postgres>, RepositoryError> {
        self.pool.begin().await.map_err(backend)
    }
}

fn sealed(r: &PgRow) -> Result<SealedRow, sqlx::Error> {
    Ok(SealedRow {
        seq: r.try_get("chain_seq")?,
        hash: r.try_get("chain_hash")?,
        version: r.try_get("chain_version")?,
        row: ChainRow {
            tenant_id: r.try_get("tenant_id")?,
            id: r.try_get("id")?,
            event_type: r.try_get("event_type")?,
            admin_id: r.try_get("admin_id")?,
            email_attempted: r.try_get("email_attempted")?,
            ip: r.try_get("ip")?,
            user_agent: r.try_get("user_agent")?,
            occurred_at_micros: r.try_get("micros")?,
            action: r.try_get("action")?,
            resource: r.try_get("resource")?,
            service_account: r.try_get("service_account")?,
            job_id: r.try_get("job_id")?,
            purpose: r.try_get("purpose")?,
            reference: r.try_get("reference")?,
            rows_shown: r.try_get("rows")?,
            fields_shown: r.try_get("fields_shown")?,
        },
    })
}

#[async_trait]
impl AuditChainRows for PgAuditChain {
    async fn tenants(&self) -> Result<Vec<String>, RepositoryError> {
        // The one chain: the main tenant's, whose id its rows still hash.
        Ok(vec![MAIN.to_string()])
    }

    async fn tail(&self, tenant: &str) -> Result<ChainTail, RepositoryError> {
        let mut tx = self.begin().await?;
        // One statement, one snapshot: the head and the last row agree
        // unless the chain was changed.
        let row = sqlx::query(
            "SELECT (SELECT max(chain_seq) FROM admin_audit_events WHERE tenant_id = $1::uuid) \
                    AS last_row, h.seq, h.hash, h.adopted_seq \
               FROM (SELECT 1) AS one \
               LEFT JOIN audit_chain_heads h ON h.tenant_id = $1::uuid",
        )
        .bind(tenant)
        .fetch_one(&mut *tx)
        .await
        .map_err(backend)?;
        tx.commit().await.map_err(backend)?;
        let seq: Option<i64> = row.try_get("seq").map_err(backend)?;
        let head = match seq {
            Some(seq) => Some(ChainHead {
                seq,
                hash: row.try_get("hash").map_err(backend)?,
                adopted_seq: row.try_get("adopted_seq").map_err(backend)?,
            }),
            None => None,
        };
        Ok(ChainTail {
            head,
            last_row: row.try_get("last_row").map_err(backend)?,
        })
    }

    async fn rows(
        &self,
        tenant: &str,
        after: i64,
        through: i64,
        limit: u32,
    ) -> Result<Vec<SealedRow>, RepositoryError> {
        let mut tx = self.begin().await?;
        let rows = sqlx::query(ROWS)
            .bind(tenant)
            .bind(after)
            .bind(through)
            .bind(i64::from(limit))
            .fetch_all(&mut *tx)
            .await
            .map_err(backend)?;
        tx.commit().await.map_err(backend)?;
        rows.iter()
            .map(sealed)
            .collect::<Result<_, _>>()
            .map_err(backend)
    }
}

#[async_trait]
impl AuditChainChecks for PgAuditChain {
    async fn record(&self, report: &ChainReport) -> Result<(), RepositoryError> {
        let mut tx = self.begin().await?;
        let broken = report.first_break;
        sqlx::query(
            "INSERT INTO audit_chain_checks (tenant_id, checked_at, rows_checked, first_seq, \
             last_seq, result, break_seq, break_kind) \
             VALUES ($1::uuid, now(), $2, $3, $4, $5, $6, $7)",
        )
        .bind(&report.tenant)
        .bind(report.rows_checked)
        .bind(report.first_seq)
        .bind(report.last_seq)
        .bind(if broken.is_some() { "break" } else { "ok" })
        .bind(broken.map(|b| b.seq))
        .bind(broken.map(|b| b.kind.as_str()))
        .execute(&mut *tx)
        .await
        .map_err(backend)?;
        tx.commit().await.map_err(backend)
    }
}
