//! The system settings' sections in Postgres (`system_settings`). The
//! composition root connects it to the application's port.

use chrono::{DateTime, Utc};
use sqlx::postgres::PgPool;
use sqlx::Row;

/// The settings sections in Postgres.
#[derive(Debug, Clone)]
pub struct PgSettingsStore {
    pool: PgPool,
}

impl PgSettingsStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// A stored section.
#[derive(Debug, Clone)]
pub struct SettingsRow {
    /// The document, with its secrets already encrypted.
    pub data: serde_json::Value,
    pub updated_at: DateTime<Utc>,
    /// The email of whoever saved it, if the account still exists.
    pub updated_by: Option<String>,
}

fn stored(row: &sqlx::postgres::PgRow) -> Result<SettingsRow, String> {
    let get_err = |e: sqlx::Error| e.to_string();
    Ok(SettingsRow {
        data: row.try_get("data").map_err(get_err)?,
        updated_at: row
            .try_get::<DateTime<Utc>, _>("updated_at")
            .map_err(get_err)?,
        updated_by: row.try_get("updated_by").map_err(get_err)?,
    })
}

impl PgSettingsStore {
    pub async fn load(&self, section: &str) -> Result<Option<SettingsRow>, String> {
        let row = sqlx::query(
            "SELECT s.data, s.updated_at, a.email AS updated_by \
             FROM system_settings s LEFT JOIN admin_users a ON a.id = s.updated_by \
             WHERE s.section = $1",
        )
        .bind(section)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.as_ref().map(stored).transpose()
    }

    /// Save (or replace) a section; `updated_by` is the admin's id (empty:
    /// the platform).
    pub async fn save(
        &self,
        section: &str,
        data: serde_json::Value,
        updated_by: &str,
    ) -> Result<SettingsRow, String> {
        let row = sqlx::query(
            "WITH saved AS ( \
                 INSERT INTO system_settings (section, data, updated_at, updated_by) \
                 VALUES ($1, $2, now(), NULLIF($3, '')::uuid) \
                 ON CONFLICT (tenant_id, section) DO UPDATE \
                 SET data = EXCLUDED.data, updated_at = now(), updated_by = EXCLUDED.updated_by \
                 RETURNING data, updated_at, updated_by) \
             SELECT s.data, s.updated_at, a.email AS updated_by \
             FROM saved s LEFT JOIN admin_users a ON a.id = s.updated_by",
        )
        .bind(section)
        .bind(&data)
        .bind(updated_by)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        stored(&row)
    }

    pub async fn delete(&self, section: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM system_settings WHERE section = $1")
            .bind(section)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
