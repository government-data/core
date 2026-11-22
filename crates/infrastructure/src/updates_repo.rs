//! The updates' maintenance in Postgres (`orq_maintenance`), the daemon's
//! report and the policy. The composition root connects it to the
//! application's port.

use chrono::{DateTime, Utc};
use sqlx::postgres::PgPool;
use sqlx::Row;

/// Maintenance, the report and the policy, in Postgres.
#[derive(Debug, Clone)]
pub struct PgMaintenanceStore {
    pool: PgPool,
}

/// The stored maintenance: reason, from and until.
pub type MaintenanceRow = (String, DateTime<Utc>, DateTime<Utc>);

/// The updates row: the daemon's report, the policy, the last "Update now"
/// (id, when, whose email) and what was announced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpdaterRecord {
    pub report: Option<(serde_json::Value, DateTime<Utc>)>,
    pub policy: Option<(serde_json::Value, DateTime<Utc>, Option<String>)>,
    pub request: Option<(String, DateTime<Utc>, Option<String>)>,
    pub notified_available: Option<String>,
    pub notified_result: Option<String>,
}

/// A line of the history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRecord {
    pub kind: String,
    pub version: String,
    pub from_version: String,
    pub detail: String,
    pub at: DateTime<Utc>,
}

fn db(e: sqlx::Error) -> String {
    format!("base de datos: {e}")
}

impl PgMaintenanceStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The platform's time zone as saved (`public.platform_timezone()`,
    /// decision 0079), `None` when it never was.
    pub async fn platform_timezone(&self) -> Result<Option<String>, String> {
        sqlx::query_scalar("SELECT public.platform_timezone()")
            .fetch_one(&self.pool)
            .await
            .map_err(db)
    }

    pub async fn maintenance(&self) -> Result<Option<MaintenanceRow>, String> {
        sqlx::query_as("SELECT reason, started_at, until FROM orq_maintenance")
            .fetch_optional(&self.pool)
            .await
            .map_err(db)
    }

    /// Enter or renew: if it was already active it keeps when it started.
    pub async fn begin(
        &self,
        reason: &str,
        until: DateTime<Utc>,
    ) -> Result<MaintenanceRow, String> {
        sqlx::query_as(
            "INSERT INTO orq_maintenance (reason, until) VALUES ($1, $2)
             ON CONFLICT (id) DO UPDATE SET
                 reason = EXCLUDED.reason,
                 until = EXCLUDED.until,
                 started_at = CASE WHEN orq_maintenance.until > now()
                                   THEN orq_maintenance.started_at ELSE now() END
             RETURNING reason, started_at, until",
        )
        .bind(reason)
        .bind(until)
        .fetch_one(&self.pool)
        .await
        .map_err(db)
    }

    pub async fn end(&self) -> Result<(), String> {
        sqlx::query("UPDATE orq_maintenance SET until = now() WHERE until > now()")
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn updater(&self) -> Result<UpdaterRecord, String> {
        let Some(r) = sqlx::query(
            "SELECT u.report, u.reported_at, u.policy, u.policy_updated_at,
                    pa.email AS policy_by, u.request_id::text AS request_id, u.requested_at,
                    COALESCE(ra.email, u.requested_by_machine) AS request_by,
                    u.notified_available, u.notified_result
               FROM orq_updater u
               LEFT JOIN admin_users pa ON pa.id = u.policy_updated_by
               LEFT JOIN admin_users ra ON ra.id = u.requested_by",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?
        else {
            return Ok(UpdaterRecord::default());
        };
        let report: Option<serde_json::Value> = r.try_get("report").map_err(db)?;
        let reported_at: Option<DateTime<Utc>> = r.try_get("reported_at").map_err(db)?;
        let policy: Option<serde_json::Value> = r.try_get("policy").map_err(db)?;
        let policy_at: Option<DateTime<Utc>> = r.try_get("policy_updated_at").map_err(db)?;
        let request: Option<String> = r.try_get("request_id").map_err(db)?;
        let requested_at: Option<DateTime<Utc>> = r.try_get("requested_at").map_err(db)?;
        Ok(UpdaterRecord {
            report: report.zip(reported_at),
            policy: policy
                .zip(policy_at)
                .map(|(p, at)| Ok::<_, String>((p, at, r.try_get("policy_by").map_err(db)?)))
                .transpose()?,
            request: request
                .zip(requested_at)
                .map(|(id, at)| Ok::<_, String>((id, at, r.try_get("request_by").map_err(db)?)))
                .transpose()?,
            notified_available: r.try_get("notified_available").map_err(db)?,
            notified_result: r.try_get("notified_result").map_err(db)?,
        })
    }

    pub async fn save_report(&self, report: &serde_json::Value) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO orq_updater (report, reported_at) VALUES ($1, now())
             ON CONFLICT (id) DO UPDATE SET report = EXCLUDED.report, reported_at = now()",
        )
        .bind(report)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Replace the installed modules (a JSON list, decision 0003).
    pub async fn save_modules(&self, modules: &serde_json::Value) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO orq_updater (modules, modules_at) VALUES ($1, now())
             ON CONFLICT (id) DO UPDATE SET modules = EXCLUDED.modules, modules_at = now()",
        )
        .bind(modules)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    /// The installed modules as last saved, `None` when they never were.
    pub async fn modules(&self) -> Result<Option<serde_json::Value>, String> {
        let modules: Option<Option<serde_json::Value>> =
            sqlx::query_scalar("SELECT modules FROM orq_updater")
                .fetch_optional(&self.pool)
                .await
                .map_err(db)?;
        Ok(modules.flatten())
    }

    pub async fn save_policy(
        &self,
        policy: &serde_json::Value,
        admin_id: &str,
    ) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO orq_updater (policy, policy_updated_at, policy_updated_by)
             VALUES ($1, now(), $2::uuid)
             ON CONFLICT (id) DO UPDATE SET policy = EXCLUDED.policy,
                 policy_updated_at = now(), policy_updated_by = EXCLUDED.policy_updated_by",
        )
        .bind(policy)
        .bind(admin_id)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    /// A new request by an administrator or, from the server, the update
    /// agent's machine: id, when and whose email (or the machine's name).
    pub async fn new_request(
        &self,
        admin_id: Option<&str>,
        machine: Option<&str>,
    ) -> Result<(String, DateTime<Utc>, Option<String>), String> {
        let r = sqlx::query(
            "WITH request AS (
                 INSERT INTO orq_updater
                     (request_id, requested_at, requested_by, requested_by_machine)
                 VALUES (gen_random_uuid(), now(), $1::uuid, $2)
                 ON CONFLICT (id) DO UPDATE SET request_id = EXCLUDED.request_id,
                     requested_at = EXCLUDED.requested_at, requested_by = EXCLUDED.requested_by,
                     requested_by_machine = EXCLUDED.requested_by_machine
                 RETURNING request_id, requested_at, requested_by, requested_by_machine
             )
             SELECT p.request_id::text AS id, p.requested_at,
                    COALESCE(a.email, p.requested_by_machine) AS email
               FROM request p LEFT JOIN admin_users a ON a.id = p.requested_by",
        )
        .bind(admin_id)
        .bind(machine)
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;
        Ok((
            r.try_get("id").map_err(db)?,
            r.try_get("requested_at").map_err(db)?,
            r.try_get("email").map_err(db)?,
        ))
    }

    pub async fn mark_notified(
        &self,
        available: Option<&str>,
        result: Option<&str>,
    ) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO orq_updater (notified_available, notified_result) VALUES ($1, $2)
             ON CONFLICT (id) DO UPDATE SET
                 notified_available = COALESCE(EXCLUDED.notified_available, orq_updater.notified_available),
                 notified_result = COALESCE(EXCLUDED.notified_result, orq_updater.notified_result)",
        )
        .bind(available)
        .bind(result)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn add_history(&self, e: &HistoryRecord) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO orq_update_history (kind, version, from_version, detail, at)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (at, version, kind) DO NOTHING",
        )
        .bind(&e.kind)
        .bind(&e.version)
        .bind(&e.from_version)
        .bind(&e.detail)
        .bind(e.at)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn history(&self, limit: u32) -> Result<Vec<HistoryRecord>, String> {
        let rows = sqlx::query(
            "SELECT kind, version, from_version, detail, at
               FROM orq_update_history ORDER BY at DESC, id DESC LIMIT $1",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.iter()
            .map(|r| {
                Ok(HistoryRecord {
                    kind: r.try_get("kind").map_err(db)?,
                    version: r.try_get("version").map_err(db)?,
                    from_version: r.try_get("from_version").map_err(db)?,
                    detail: r.try_get("detail").map_err(db)?,
                    at: r.try_get("at").map_err(db)?,
                })
            })
            .collect()
    }
}
