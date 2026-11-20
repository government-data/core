//! The lake's pools in Postgres (`lake_pools`, 0066 step 11, the user's
//! ruling 1 of 2026-11-14): one row per pool, defined again in place.

use std::time::SystemTime;

use async_trait::async_trait;
use auth_gateway::{LakePool, LakePoolStore, PoolLayout};
use chrono::{DateTime, Utc};
use domain::{AdminId, RepositoryError};
use sqlx::postgres::{PgPool, PgRow};
use sqlx::Row;

use super::backend;

/// A Postgres-backed [`LakePoolStore`].
#[derive(Debug, Clone)]
pub struct PgLakePools {
    pool: PgPool,
}

impl PgLakePools {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const COLUMNS: &str = "name, nodes, drives, defined_by::text AS defined_by, defined_at";

/// A row read back, checked again as the use case checked it.
fn pool(row: &PgRow) -> Result<LakePool, RepositoryError> {
    let name: String = row.try_get("name").map_err(backend)?;
    let nodes: Vec<String> = row.try_get("nodes").map_err(backend)?;
    let drives: i32 = row.try_get("drives").map_err(backend)?;
    let drives = u16::try_from(drives).unwrap_or(0);
    let layout = PoolLayout::parse(&name, &nodes, drives)
        .map_err(|e| RepositoryError::Backend(format!("pool {name:?}: {e:?}")))?;
    let at: DateTime<Utc> = row.try_get("defined_at").map_err(backend)?;
    Ok(LakePool {
        layout,
        defined_by: AdminId::new(row.try_get::<String, _>("defined_by").map_err(backend)?),
        defined_at: SystemTime::from(at),
    })
}

#[async_trait]
impl LakePoolStore for PgLakePools {
    async fn define(
        &self,
        layout: &PoolLayout,
        by: &AdminId,
        at: SystemTime,
    ) -> Result<LakePool, RepositoryError> {
        let row = sqlx::query(&format!(
            "INSERT INTO lake_pools (name, nodes, drives, defined_by, defined_at) \
             VALUES ($1, $2, $3, $4::uuid, $5) \
             ON CONFLICT (tenant_id, name) DO UPDATE SET nodes = EXCLUDED.nodes, \
             drives = EXCLUDED.drives, defined_by = EXCLUDED.defined_by, \
             defined_at = EXCLUDED.defined_at \
             RETURNING {COLUMNS}"
        ))
        .bind(layout.name())
        .bind(layout.nodes())
        .bind(i32::from(layout.drives()))
        .bind(by.as_str())
        .bind(DateTime::<Utc>::from(at))
        .fetch_one(&self.pool)
        .await
        .map_err(backend)?;
        pool(&row)
    }

    async fn find(&self, name: &str) -> Result<Option<LakePool>, RepositoryError> {
        let sql = format!("SELECT {COLUMNS} FROM lake_pools WHERE name = $1");
        let row = sqlx::query(&sql)
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?;
        row.as_ref().map(pool).transpose()
    }

    async fn all(&self) -> Result<Vec<LakePool>, RepositoryError> {
        let sql = format!("SELECT {COLUMNS} FROM lake_pools ORDER BY name");
        let rows = sqlx::query(&sql)
            .fetch_all(&self.pool)
            .await
            .map_err(backend)?;
        rows.iter().map(pool).collect()
    }
}
