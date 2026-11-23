//! Asking for a map of a source's table (decisions 0038 and 0053), in the
//! module's `user_router`: `POST /quality/sources/{name}/maps` queues a
//! `map` job for one table and the dataset it becomes, a name `main` does
//! not have; its report publishes the dataset and opens its first cut.
//! `GET /quality/maps/{job}` says how far it went. Behind
//! `quality:ManageDatasets` on `dataset/<dataset>`, asked of the request's
//! [`Caller`], and the map audited.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use contracts::{AuditEntry, Caller, Change};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

use crate::catalog::Catalog;
use crate::dataset_api::{recent_cuts, CutOut};
use crate::firing::queue_job;
use crate::mapping::ReadFrom;
use crate::proposal::Patterns;
use crate::source::valid_name;
use crate::source_store;

pub const MANAGE: &str = "quality:ManageDatasets";
/// The largest sample a map may ask for.
const MAX_SAMPLE: u64 = 1_000_000;

#[derive(Clone)]
struct MapApi {
    pool: PgPool,
    patterns: Patterns,
    catalog: Option<Arc<dyn Catalog>>,
}

/// `POST /quality/sources/{name}/maps` and `GET /quality/maps/{job}`; each
/// job queued carries `patterns`, the installation's, so its profile and
/// its proposal use the same.
pub fn routes(pool: PgPool, patterns: Patterns, catalog: Option<Arc<dyn Catalog>>) -> Router {
    Router::new()
        .route("/quality/sources/{name}/maps", post(ask))
        .route("/quality/maps/{job}", get(progress))
        .with_state(MapApi {
            pool,
            patterns,
            catalog,
        })
}

/// The route's OpenAPI path and schemas.
#[derive(Debug, OpenApi)]
#[openapi(
    paths(ask, progress),
    components(schemas(MapBody, MapOut, MapProgress)),
    tags((name = "maps", description = "Proposing a dataset from a source's table")),
)]
pub struct MapDoc;

/// Which table to map, and the dataset it becomes.
#[derive(Debug, Deserialize, ToSchema)]
pub struct MapBody {
    /// `table` or `schema.table`, as the source names it.
    pub table: String,
    /// The dataset's name: `a-z`, `0-9`, `_`, starting with a letter.
    pub dataset: String,
    /// Rows to profile; 10,000 when left out.
    pub sample: Option<u64>,
}

/// The map job queued, and the dataset it makes.
#[derive(Debug, Serialize, ToSchema)]
pub struct MapOut {
    #[schema(value_type = String)]
    pub job: Uuid,
    pub dataset: String,
}

/// How far a map went: its job, then the dataset's first cut.
#[derive(Debug, Serialize, ToSchema)]
pub struct MapProgress {
    pub dataset: String,
    /// The map job's state: `queued`, `running`, `done` or `failed`.
    pub state: String,
    /// What the job said when it failed (the worker's words, no value).
    pub error: Option<String>,
    /// Once mapped, the dataset's latest cut.
    pub cut: Option<CutOut>,
}

fn refuse(code: StatusCode, error: &str) -> Response {
    (code, Json(json!({ "error": error }))).into_response()
}

/// Queues the map of a source's table.
#[utoipa::path(
    post,
    path = "/quality/sources/{name}/maps",
    operation_id = "quality_map_table",
    params(("name" = String, Path, description = "The source's name")),
    request_body = MapBody,
    responses(
        (status = 201, description = "The map job queued", body = MapOut),
        (status = 401, description = "No valid session"),
        (status = 403, description = "May not manage this dataset"),
        (status = 404, description = "No such source"),
        (status = 409, description = "`dataset_exists`: the catalog has that dataset"),
        (status = 422, description = "`dataset`, `table` or `sample` does not hold"),
        (status = 503, description = "No catalog to know the dataset"),
    ),
    tag = "maps",
)]
async fn ask(
    State(MapApi {
        pool,
        patterns,
        catalog,
    }): State<MapApi>,
    Extension(caller): Extension<Caller>,
    Path(name): Path<String>,
    Json(body): Json<MapBody>,
) -> Response {
    let resource = format!("dataset/{}", body.dataset);
    if !caller.can(MANAGE, &resource) {
        return refuse(StatusCode::FORBIDDEN, "forbidden");
    }
    if !valid_name(&body.dataset) {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, "dataset");
    }
    let from = ReadFrom {
        source: name.clone(),
        table: body.table.clone(),
    };
    if !from.is_valid() {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, "table");
    }
    let sample = body.sample.unwrap_or(10_000);
    if sample == 0 || sample > MAX_SAMPLE {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, "sample");
    }
    match source_store::exists(&pool, &name).await {
        Ok(true) => {}
        Ok(false) => return refuse(StatusCode::NOT_FOUND, "no_such_source"),
        Err(e) => {
            tracing::error!("map: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    // A map publishes on `main`: never over a dataset that is there.
    let Some(catalog) = catalog else {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "catalog_unavailable");
    };
    match catalog.snapshot(&body.dataset).await {
        Ok(None) => {}
        Ok(Some(_)) => return refuse(StatusCode::CONFLICT, "dataset_exists"),
        Err(e) => {
            tracing::error!("map: catalog: {e}");
            return refuse(StatusCode::SERVICE_UNAVAILABLE, "catalog_unavailable");
        }
    }
    let parameters = json!({
        "source": name,
        "table": body.table,
        "sample": sample,
        "patterns": patterns,
        "requested_by": caller.account,
    });
    let queued = async {
        let mut tx = pool.begin().await?;
        let job = queue_job(&mut tx, "map", Some(&body.dataset), &parameters).await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(job)
    };
    let job = match queued.await {
        Ok(job) => job,
        Err(e) => {
            tracing::error!("map: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let entry = AuditEntry {
        action: MANAGE,
        resource,
        change: Change::Ran,
    };
    if let Err(e) = caller.audit(entry).await {
        tracing::error!("map: audit: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let out = MapOut {
        job,
        dataset: body.dataset,
    };
    (StatusCode::CREATED, Json(out)).into_response()
}

/// How far the map `job` went.
#[utoipa::path(
    get,
    path = "/quality/maps/{job}",
    operation_id = "quality_map_progress",
    params(("job" = String, Path, description = "The map job")),
    responses(
        (status = 200, description = "The map and its first cut", body = MapProgress),
        (status = 401, description = "No valid session"),
        (status = 403, description = "May not manage its dataset"),
        (status = 404, description = "No such map"),
    ),
    tag = "maps",
)]
async fn progress(
    State(api): State<MapApi>,
    Extension(caller): Extension<Caller>,
    Path(job): Path<Uuid>,
) -> Response {
    let row: Result<Option<(String, String, Option<String>)>, sqlx::Error> = sqlx::query_as(
        "SELECT dataset, state, error FROM quality.jobs WHERE id = $1 AND kind = 'map'",
    )
    .bind(job)
    .fetch_optional(&api.pool)
    .await;
    let (dataset, state, error) = match row {
        Ok(Some(row)) => row,
        Ok(None) => return refuse(StatusCode::NOT_FOUND, "no_such_map"),
        Err(e) => {
            tracing::error!("map progress: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if !caller.can(MANAGE, &format!("dataset/{dataset}")) {
        return refuse(StatusCode::FORBIDDEN, "forbidden");
    }
    let cut = if state == "done" {
        match recent_cuts(&api.pool, &dataset, 1).await {
            Ok(cuts) => cuts.into_iter().next(),
            Err(e) => {
                tracing::error!("map progress: {e}");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    } else {
        None
    };
    let out = MapProgress {
        dataset,
        error: (state == "failed").then_some(error).flatten(),
        state,
        cut,
    };
    Json(out).into_response()
}
