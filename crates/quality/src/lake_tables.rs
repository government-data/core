//! The lake listed as a source (decision 0043, point 1): a `lake_tables`
//! job lists the warehouse, its namespaces as schemas and each
//! table with its columns, from the tables' metadata with credentials that
//! read no data file (`LakeGrant::metadata`), never a row; the installation
//! keeps its last listing, which a late report of an older job never
//! replaces. `GET /quality/lake/tables` gives it to whoever may view some
//! rules; `POST` lists again, behind `quality:ManageSources` on some
//! source, audited.

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{extract::State, Extension, Json, Router};
use contracts::{AuditEntry, Caller, Change};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use utoipa::OpenApi;
use uuid::Uuid;

use crate::firing::queue_job;
use crate::job_sources;
use crate::lake::{self, LakeAccess};
use crate::refusal::refuse;
use crate::source_api::MANAGE;
use crate::source_tables::{assemble, Kept, Latest, ListingView, SourceTable, MAX_TABLES};
use crate::tables_api::ListingQueued;
use crate::worker_api::WorkerApi;

/// The job's kind.
pub const KIND: &str = "lake_tables";
/// Who may read the lake's listing: whoever may view some rules.
const VIEW_RULES: &str = "quality:ViewRules";
/// The longest namespace, table or column name a listing takes.
const MAX_NAME: usize = 255;

/// What a `lake_tables` job reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LakeListing {
    pub tables: Vec<SourceTable>,
    /// More than [`MAX_TABLES`] were there.
    #[serde(default)]
    pub truncated: bool,
}

impl LakeListing {
    /// At most [`MAX_TABLES`], each named and its columns too.
    pub fn is_valid(&self) -> bool {
        let named = |s: &str| !s.is_empty() && s.chars().count() <= MAX_NAME;
        self.tables.len() <= MAX_TABLES
            && self.tables.iter().all(|t| {
                named(&t.schema)
                    && named(&t.name)
                    && t.columns
                        .iter()
                        .all(|c| named(&c.name) && !c.data_type.is_empty())
            })
    }
}

/// `GET` and `POST /quality/lake/tables`.
pub fn routes(pool: PgPool) -> Router {
    Router::new()
        .route("/quality/lake/tables", get(listing).post(refresh))
        .with_state(pool)
}

/// The routes' OpenAPI paths.
#[derive(Debug, OpenApi)]
#[openapi(paths(listing, refresh))]
pub struct LakeTablesDoc;

fn internal(e: &sqlx::Error) -> Response {
    tracing::error!("lake tables: {e}");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

/// The lake's last listing, and whether a newer one is coming.
pub async fn view(pool: &PgPool) -> Result<ListingView, sqlx::Error> {
    let latest: Option<Latest> = sqlx::query_as(
        "SELECT id, state, attempts, max_attempts, error FROM quality.jobs
          WHERE kind = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(KIND)
    .fetch_optional(pool)
    .await?;
    let kept: Option<Kept> =
        sqlx::query_as("SELECT job, listed_at, tables, truncated FROM quality.lake_tables")
            .fetch_optional(pool)
            .await?;
    Ok(assemble(latest, kept))
}

/// Keeps job `job`'s listing as the lake's last, unless a newer job's is.
pub(crate) async fn store(
    pool: &PgPool,
    job: Uuid,
    listing: &LakeListing,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO quality.lake_tables (job, asked_at, tables, truncated)
         SELECT j.id, j.created_at, $2, $3 FROM quality.jobs j WHERE j.id = $1
         ON CONFLICT (tenant_id) DO UPDATE
            SET job = EXCLUDED.job, asked_at = EXCLUDED.asked_at, listed_at = now(),
                tables = EXCLUDED.tables, truncated = EXCLUDED.truncated
          WHERE quality.lake_tables.asked_at <= EXCLUDED.asked_at",
    )
    .bind(job)
    .bind(sqlx::types::Json(&listing.tables))
    .bind(listing.truncated)
    .execute(pool)
    .await
    .map(|_| ())
}

/// A listing job's credentials, recorded first: the warehouse's metadata,
/// no data file; none without an issuer.
pub(crate) async fn access(
    api: &WorkerApi,
    job: Uuid,
    client: &str,
    headers: &HeaderMap,
) -> Result<Option<LakeAccess>, StatusCode> {
    let Some(issuer) = &api.services.lake else {
        return Ok(None);
    };
    let warehouse = lake::WAREHOUSE.to_string();
    job_sources::record(
        api.services.issues.as_deref(),
        job,
        KIND,
        client,
        headers,
        &format!("warehouse/{warehouse}/metadata"),
    )
    .await?;
    issuer
        .issue(
            &format!("job-{job}"),
            &lake::LakeGrant::metadata(&warehouse),
        )
        .await
        .map(Some)
        .map_err(|e| {
            tracing::error!("lake credentials: {e}");
            StatusCode::SERVICE_UNAVAILABLE
        })
}

/// A `lake_tables` job's report, kept if it holds together.
pub(crate) async fn report(
    pool: &PgPool,
    job: Uuid,
    body: serde_json::Value,
) -> Result<StatusCode, StatusCode> {
    let listing: LakeListing =
        serde_json::from_value(body).map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    if !listing.is_valid() {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    store(pool, job, &listing).await.map_err(|e| {
        tracing::error!("lake tables report: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// The lake's last listing, names and columns only.
#[utoipa::path(get, path = "/quality/lake/tables", operation_id = "quality_lake_tables",
    tag = "sources",
    responses((status = 200, description = "Its last listing", body = ListingView),
        (status = 401), (status = 403, description = "May view no rules")))]
async fn listing(State(pool): State<PgPool>, Extension(caller): Extension<Caller>) -> Response {
    if !caller.can_some(VIEW_RULES) {
        return refuse(StatusCode::FORBIDDEN, "forbidden");
    }
    match view(&pool).await {
        Ok(view) => Json(view).into_response(),
        Err(e) => internal(&e),
    }
}

/// Lists the lake again.
#[utoipa::path(post, path = "/quality/lake/tables", operation_id = "quality_list_lake_tables",
    tag = "sources",
    responses((status = 202, description = "The listing job queued", body = ListingQueued),
        (status = 401), (status = 403, description = "May manage no source")))]
async fn refresh(State(pool): State<PgPool>, Extension(caller): Extension<Caller>) -> Response {
    if !caller.can_some(MANAGE) {
        return refuse(StatusCode::FORBIDDEN, "forbidden");
    }
    let queued = async {
        let mut tx = pool.begin().await?;
        let job = queue_job(&mut tx, KIND, None, &json!({})).await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(job)
    };
    let job = match queued.await {
        Ok(job) => job,
        Err(e) => return internal(&e),
    };
    let entry = AuditEntry {
        action: MANAGE,
        resource: "lake".into(),
        change: Change::Ran,
    };
    if let Err(e) = caller.audit(entry).await {
        tracing::error!("lake tables: audit: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    (StatusCode::ACCEPTED, Json(ListingQueued { job })).into_response()
}
