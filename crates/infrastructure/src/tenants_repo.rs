//! The tenant registry (`tenants`). The table has no RLS: it is read before
//! knowing which tenant the request belongs to.

use async_trait::async_trait;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use tenancy::{Directory, Status, Tenant, TenantRecord};

#[derive(Debug, Clone)]
pub struct PgTenantDirectory {
    pool: PgPool,
}

impl PgTenantDirectory {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const COLUMNS: &str = "id::text AS id, slug, name, status, cell";

fn record(row: &PgRow) -> Result<TenantRecord, String> {
    let status: String = row.get("status");
    Ok(TenantRecord {
        tenant: Tenant {
            id: row.get("id"),
            slug: row.get("slug"),
        },
        name: row.get("name"),
        status: Status::parse(&status).ok_or_else(|| format!("unknown status: {status}"))?,
        cell: row.get("cell"),
    })
}

#[async_trait]
impl Directory for PgTenantDirectory {
    async fn by_slug(&self, slug: &str) -> Result<Option<TenantRecord>, String> {
        sqlx::query(&format!("SELECT {COLUMNS} FROM tenants WHERE slug = $1"))
            .bind(slug)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())?
            .as_ref()
            .map(record)
            .transpose()
    }

    async fn in_cell(&self, cell: &str) -> Result<Vec<TenantRecord>, String> {
        sqlx::query(&format!(
            "SELECT {COLUMNS} FROM tenants WHERE cell = $1 AND status <> 'closed' ORDER BY slug"
        ))
        .bind(cell)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?
        .iter()
        .map(record)
        .collect()
    }

    async fn with_email(&self, email: &str, cell: &str) -> Result<Vec<TenantRecord>, String> {
        // `tenants_with_email` goes through the cell's companies with each
        // one's RLS: it only returns the ones the account is in.
        sqlx::query(
            "SELECT t.id::text AS id, t.slug, t.name, t.status, t.cell \
               FROM tenants_with_email($1, $2) AS e (id) \
               JOIN tenants t ON t.id = e.id \
              ORDER BY t.name, t.slug",
        )
        .bind(email)
        .bind(cell)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?
        .iter()
        .map(record)
        .collect()
    }
}
