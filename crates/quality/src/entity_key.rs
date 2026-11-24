//! The installation's entity key (decision 0057): the HMAC key with which a
//! worker turns a client's normalized document into the hash `entity_icd`
//! keeps, so the same client has the same hash in every cut and dataset and
//! no result holds a document. Generated once from the system's random
//! source, sealed with the sources' key, given to each evaluation as its
//! other credentials are, and kept by no worker.

use std::fmt;

use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::job_sources;
use crate::source_store::Sealer;
use crate::worker_api::WorkerApi;

/// The key as an evaluation gets it, hex; never printed.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityKey(pub String);

impl fmt::Debug for EntityKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EntityKey(..)")
    }
}

/// Two random UUIDs: 244 random bits from the system's source, too many
/// for any document to be tried against a hash.
fn generate() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// The installation's key, created the
/// first time; two evaluations asking at once get the same one.
pub async fn of_installation(pool: &PgPool, sealer: &dyn Sealer) -> Result<EntityKey, String> {
    let sealed = sealer.seal(&generate())?;
    sqlx::query("INSERT INTO quality.entity_keys (sealed) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(sealed)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    let (kept,): (String,) = sqlx::query_as("SELECT sealed FROM quality.entity_keys")
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;
    sealer.open(&kept).map(EntityKey)
}

/// An `evaluate` job's key, recorded as issued first, as every credential
/// a job gets (0030); other jobs, and an installation without the sources'
/// key, get none.
pub(crate) async fn for_job(
    api: &WorkerApi,
    job: Uuid,
    kind: &str,
    client: &str,
    headers: &HeaderMap,
) -> Result<Option<EntityKey>, StatusCode> {
    let Some(sealer) = api
        .services
        .sources
        .as_deref()
        .filter(|_| kind == "evaluate")
    else {
        return Ok(None);
    };
    let log = api.services.issues.as_deref();
    job_sources::record(log, job, kind, client, headers, "entity_key").await?;
    of_installation(&api.pool, sealer)
        .await
        .map(Some)
        .map_err(|e| {
            tracing::error!("entity key: {e}");
            StatusCode::SERVICE_UNAVAILABLE
        })
}
