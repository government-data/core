//! The sources' store (decision 0036), the module's own adapter as the queue
//! is, inside the tenant's `tenancy::within`. The secret fields are sealed
//! by a [`Sealer`] as one JSON object, in two versions: writing a secret
//! moves the current version to the previous one; a secret left out of an
//! edit keeps its value. What is read back for people never has a secret,
//! only which ones are set.

use std::collections::BTreeMap;

use serde::Serialize;
use sqlx::PgPool;

use crate::source::{Source, SourceError};

/// Seals and opens secrets (the installation's key, `secret_box`).
pub trait Sealer: Send + Sync {
    fn seal(&self, plain: &str) -> Result<String, String>;
    fn open(&self, sealed: &str) -> Result<String, String>;
}

/// Why a source was not stored or read.
#[derive(Debug)]
pub enum StoreError {
    /// The source does not hold together: every reason.
    Invalid(Vec<SourceError>),
    /// No source by that name.
    NotFound,
    /// The key could not seal or open its secrets.
    Seal,
    Db(sqlx::Error),
}

impl From<sqlx::Error> for StoreError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

/// A source as people see it: its secrets only by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct SourceView {
    pub name: String,
    pub driver: String,
    pub fields: BTreeMap<String, String>,
    /// The secret fields that are set.
    pub secrets: Vec<String>,
    /// When the current secrets were written (RFC 3339).
    pub secrets_set_at: String,
    /// Whether a previous version is kept.
    pub previous: bool,
}

/// A source with its secrets, for one job: the current version, and the
/// previous one if there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub source: Source,
    pub previous: Option<BTreeMap<String, String>>,
}

type Secrets = BTreeMap<String, String>;

#[derive(sqlx::FromRow)]
struct Row {
    name: String,
    driver: String,
    fields: sqlx::types::Json<BTreeMap<String, String>>,
    secrets: String,
    secrets_set_at: chrono::DateTime<chrono::Utc>,
    previous_secrets: Option<String>,
}

const COLUMNS: &str = "name, driver, fields, secrets, secrets_set_at, previous_secrets";

fn open(sealer: &dyn Sealer, sealed: &str) -> Result<Secrets, StoreError> {
    let plain = sealer.open(sealed).map_err(|_| StoreError::Seal)?;
    serde_json::from_str(&plain).map_err(|_| StoreError::Seal)
}

fn seal(sealer: &dyn Sealer, secrets: &Secrets) -> Result<String, StoreError> {
    let plain = serde_json::to_string(secrets).map_err(|_| StoreError::Seal)?;
    sealer.seal(&plain).map_err(|_| StoreError::Seal)
}

async fn row(pool: &PgPool, name: &str) -> Result<Option<Row>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM quality.sources WHERE name = $1"
    ))
    .bind(name)
    .fetch_optional(pool)
    .await
}

/// What [`put`] did, for the audit log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Put {
    Created,
    /// Its fields; its secrets as they were.
    Changed,
    /// Its secrets (and maybe its fields): the previous version kept.
    SecretsChanged,
}

/// Creates the source, or replaces its fields; its secrets are those
/// written merged over the current ones. Validated whole, after merging.
pub async fn put(pool: &PgPool, sealer: &dyn Sealer, source: &Source) -> Result<Put, StoreError> {
    let current = match row(pool, &source.name).await? {
        Some(r) => Some((open(sealer, &r.secrets)?, r.secrets)),
        None => None,
    };
    let mut merged = source.clone();
    if let Some((secrets, _)) = &current {
        for (key, value) in secrets {
            merged.secrets.entry(key.clone()).or_insert(value.clone());
        }
    }
    let driver = merged.validate().map_err(StoreError::Invalid)?;
    // A driver's secrets only: a field the new driver does not have goes.
    merged
        .secrets
        .retain(|k, _| driver.fields.iter().any(|f| f.key == k));
    let fields = sqlx::types::Json(&merged.fields);
    match current {
        None => {
            sqlx::query(
                "INSERT INTO quality.sources (name, driver, fields, secrets)
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(&merged.name)
            .bind(&merged.driver)
            .bind(fields)
            .bind(seal(sealer, &merged.secrets)?)
            .execute(pool)
            .await?;
            Ok(Put::Created)
        }
        Some((secrets, _)) if secrets == merged.secrets => {
            sqlx::query(
                "UPDATE quality.sources SET driver = $2, fields = $3, updated_at = now()
                  WHERE name = $1",
            )
            .bind(&merged.name)
            .bind(&merged.driver)
            .bind(fields)
            .execute(pool)
            .await?;
            Ok(Put::Changed)
        }
        Some((_, sealed)) => {
            sqlx::query(
                "UPDATE quality.sources
                    SET driver = $2, fields = $3, secrets = $4, secrets_set_at = now(),
                        previous_secrets = $5, previous_set_at = secrets_set_at,
                        updated_at = now()
                  WHERE name = $1",
            )
            .bind(&merged.name)
            .bind(&merged.driver)
            .bind(fields)
            .bind(seal(sealer, &merged.secrets)?)
            .bind(sealed)
            .execute(pool)
            .await?;
            Ok(Put::SecretsChanged)
        }
    }
}

fn view(r: Row, sealer: &dyn Sealer) -> Result<SourceView, StoreError> {
    Ok(SourceView {
        secrets: open(sealer, &r.secrets)?.into_keys().collect(),
        name: r.name,
        driver: r.driver,
        fields: r.fields.0,
        secrets_set_at: r
            .secrets_set_at
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        previous: r.previous_secrets.is_some(),
    })
}

/// The tenant's sources, by name, without their secrets.
pub async fn list(pool: &PgPool, sealer: &dyn Sealer) -> Result<Vec<SourceView>, StoreError> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM quality.sources ORDER BY name"
    ))
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(|r| view(r, sealer)).collect()
}

/// Whether the source `name` exists.
pub async fn exists(pool: &PgPool, name: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM quality.sources WHERE name = $1)")
        .bind(name)
        .fetch_one(pool)
        .await
}

/// The driver of the source `name`, if there is one.
pub async fn driver_of(pool: &PgPool, name: &str) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT driver FROM quality.sources WHERE name = $1")
        .bind(name)
        .fetch_optional(pool)
        .await
}

/// The source `name`'s row, held: locked for update (`FOR UPDATE`, to
/// delete it) or shared (`FOR SHARE`, while a draft that reads it is
/// approved), until the transaction ends. A deletion and an approval of
/// the same source go one after the other, so `main` never names a source
/// that a deletion took meanwhile. `None` if it is not there.
pub async fn hold(
    pool: &PgPool,
    name: &str,
    to_delete: bool,
) -> Result<Option<sqlx::Transaction<'static, sqlx::Postgres>>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let lock = if to_delete { "FOR UPDATE" } else { "FOR SHARE" };
    let found: Option<i32> = sqlx::query_scalar(&format!(
        "SELECT 1 FROM quality.sources WHERE name = $1 {lock}"
    ))
    .bind(name)
    .fetch_optional(&mut *tx)
    .await?;
    Ok(found.map(|_| tx))
}

/// Deletes the source held for update, and ends the hold.
pub async fn delete_held(
    mut tx: sqlx::Transaction<'static, sqlx::Postgres>,
    name: &str,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query("DELETE FROM quality.sources WHERE name = $1")
        .bind(name)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(done.rows_affected() > 0)
}

pub async fn delete(pool: &PgPool, name: &str) -> Result<bool, sqlx::Error> {
    let done = sqlx::query("DELETE FROM quality.sources WHERE name = $1")
        .bind(name)
        .execute(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// The source `name` with both versions of its secrets, for a job.
pub async fn credentials(
    pool: &PgPool,
    sealer: &dyn Sealer,
    name: &str,
) -> Result<Credentials, StoreError> {
    let r = row(pool, name).await?.ok_or(StoreError::NotFound)?;
    let previous = r
        .previous_secrets
        .as_deref()
        .map(|s| open(sealer, s))
        .transpose()?;
    Ok(Credentials {
        source: Source {
            secrets: open(sealer, &r.secrets)?,
            name: r.name,
            driver: r.driver,
            fields: r.fields.0,
        },
        previous,
    })
}
