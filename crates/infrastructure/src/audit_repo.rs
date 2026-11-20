//! Postgres adapter for the [`AuditRepository`] port, over the
//! `admin_audit_events` table.
//!
//! Timestamps cross the boundary as Unix epoch seconds, the same convention
//! [`crate::admin_repo`] uses.
//!
//! The table's trigger chains each row under its tenant's head, locked until
//! the row's transaction ends (decision 0070, point 6): each insert is its own
//! short transaction, never part of a longer one, and waits at most
//! [`LOCK_TIMEOUT`] for the head, failing as any other failed write.

use domain::{
    AdminId, AuditEvent, AuditEventType, AuditId, AuditRepository, NewActionAudit, NewAuditEvent,
    NewViewAudit, RepositoryError, ViewShown,
};
use sqlx::postgres::{PgPool, Postgres};
use sqlx::{Row, Transaction};

use crate::admin_repo::{from_epoch, to_epoch};

/// Map an arbitrary sqlx error to a backend [`RepositoryError`].
fn backend(err: sqlx::Error) -> RepositoryError {
    RepositoryError::Backend(err.to_string())
}

/// How long an audit insert waits for its tenant's chain head.
pub const LOCK_TIMEOUT: &str = "5s";

/// An insert's own transaction, with [`LOCK_TIMEOUT`].
async fn begin(pool: &PgPool) -> Result<Transaction<'static, Postgres>, RepositoryError> {
    let mut tx = pool.begin().await.map_err(backend)?;
    sqlx::query("SELECT set_config('lock_timeout', $1, true)")
        .bind(LOCK_TIMEOUT)
        .execute(&mut *tx)
        .await
        .map_err(backend)?;
    Ok(tx)
}

/// A Postgres-backed [`AuditRepository`].
#[derive(Debug, Clone)]
pub struct PgAuditRepository {
    pool: PgPool,
}

impl PgAuditRepository {
    /// Build the repository over an existing pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl AuditRepository for PgAuditRepository {
    async fn record(&self, event: &NewAuditEvent) -> Result<AuditId, RepositoryError> {
        let mut tx = begin(&self.pool).await?;
        let id: String = sqlx::query_scalar(
            "INSERT INTO admin_audit_events \
             (event_type, admin_id, email_attempted, ip, user_agent, occurred_at) \
             VALUES ($1, $2::uuid, $3, $4, $5, to_timestamp($6)) \
             RETURNING id::text",
        )
        .bind(event.event_type.as_str())
        .bind(event.admin_id.as_ref().map(AdminId::as_str))
        .bind(&event.email_attempted)
        .bind(&event.ip)
        .bind(&event.user_agent)
        .bind(to_epoch(event.occurred_at))
        .fetch_one(&mut *tx)
        .await
        .map_err(backend)?;
        tx.commit().await.map_err(backend)?;
        Ok(AuditId::new(id))
    }

    async fn record_action(&self, event: &NewActionAudit) -> Result<AuditId, RepositoryError> {
        let mut tx = begin(&self.pool).await?;
        let id: String = sqlx::query_scalar(
            "INSERT INTO admin_audit_events \
             (event_type, admin_id, service_account, action, resource, ip, user_agent, \
              occurred_at, job_id) \
             VALUES ($1, $2::uuid, $3, $4, $5, $6, $7, to_timestamp($8), $9::uuid) \
             RETURNING id::text",
        )
        .bind(event.event_type.as_str())
        .bind(event.admin_id.as_ref().map(AdminId::as_str))
        .bind(&event.service_account)
        .bind(&event.action)
        .bind(&event.resource)
        .bind(&event.ip)
        .bind(&event.user_agent)
        .bind(to_epoch(event.occurred_at))
        .bind(&event.job_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(backend)?;
        tx.commit().await.map_err(backend)?;
        Ok(AuditId::new(id))
    }

    async fn record_view(&self, event: &NewViewAudit) -> Result<AuditId, RepositoryError> {
        let (action, shown) = (&event.action, &event.shown);
        let mut tx = begin(&self.pool).await?;
        let id: String = sqlx::query_scalar(
            "INSERT INTO admin_audit_events \
             (event_type, admin_id, service_account, action, resource, ip, user_agent, \
              occurred_at, job_id, purpose, reference, rows_shown, fields_shown) \
             VALUES ($1, $2::uuid, $3, $4, $5, $6, $7, to_timestamp($8), $9::uuid, \
              $10, $11, $12, $13) \
             RETURNING id::text",
        )
        .bind(action.event_type.as_str())
        .bind(action.admin_id.as_ref().map(AdminId::as_str))
        .bind(&action.service_account)
        .bind(&action.action)
        .bind(&action.resource)
        .bind(&action.ip)
        .bind(&action.user_agent)
        .bind(to_epoch(action.occurred_at))
        .bind(&action.job_id)
        .bind(&shown.purpose)
        .bind(&shown.reference)
        .bind(i32::try_from(shown.rows_shown).unwrap_or(i32::MAX))
        .bind(&shown.fields_shown)
        .fetch_one(&mut *tx)
        .await
        .map_err(backend)?;
        tx.commit().await.map_err(backend)?;
        Ok(AuditId::new(id))
    }

    async fn list_recent(&self, limit: u32) -> Result<Vec<AuditEvent>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id::text AS id, event_type, admin_id::text AS admin_id, email_attempted, \
             ip, user_agent, EXTRACT(EPOCH FROM occurred_at)::bigint AS occurred_at_epoch, \
             action, resource, job_id::text AS job_id, service_account, \
             purpose, reference, rows_shown, fields_shown \
             FROM admin_audit_events ORDER BY occurred_at DESC LIMIT $1",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;

        rows.into_iter()
            .map(|row| {
                let id: String = row.try_get("id").map_err(backend)?;
                let event_type: String = row.try_get("event_type").map_err(backend)?;
                let admin_id: Option<String> = row.try_get("admin_id").map_err(backend)?;
                let email_attempted: Option<String> =
                    row.try_get("email_attempted").map_err(backend)?;
                let ip: String = row.try_get("ip").map_err(backend)?;
                let user_agent: Option<String> = row.try_get("user_agent").map_err(backend)?;
                let occurred_at: i64 = row.try_get("occurred_at_epoch").map_err(backend)?;
                let action: Option<String> = row.try_get("action").map_err(backend)?;
                let resource: Option<String> = row.try_get("resource").map_err(backend)?;
                let job_id: Option<String> = row.try_get("job_id").map_err(backend)?;
                let service_account: Option<String> =
                    row.try_get("service_account").map_err(backend)?;
                let view = view_shown(&row)?;

                let event_type = AuditEventType::parse(&event_type)
                    .map_err(|e| RepositoryError::Backend(format!("stored event_type: {e}")))?;

                Ok(AuditEvent {
                    id: AuditId::new(id),
                    event_type,
                    admin_id: admin_id.map(AdminId::new),
                    email_attempted,
                    ip,
                    user_agent,
                    occurred_at: from_epoch(occurred_at),
                    action,
                    resource,
                    job_id,
                    service_account,
                    view,
                })
            })
            .collect()
    }
}

/// A view's reason and shape, on a row that has one (decision 0069): a row
/// without a purpose is not a view, or one recorded before it.
fn view_shown(row: &sqlx::postgres::PgRow) -> Result<Option<ViewShown>, RepositoryError> {
    let Some(purpose) = row
        .try_get::<Option<String>, _>("purpose")
        .map_err(backend)?
    else {
        return Ok(None);
    };
    let rows_shown: Option<i32> = row.try_get("rows_shown").map_err(backend)?;
    let fields_shown: Option<Vec<String>> = row.try_get("fields_shown").map_err(backend)?;
    Ok(Some(ViewShown {
        purpose,
        reference: row.try_get("reference").map_err(backend)?,
        rows_shown: rows_shown.and_then(|n| u32::try_from(n).ok()).unwrap_or(0),
        fields_shown: fields_shown.unwrap_or_default(),
    }))
}
