//! _Run now_ (decision 0035, point 2): a dataset's cut opened by a person,
//! as a schedule's fire opens one: today's date in the installation's
//! timezone, the same choice between blocking and replacing a cut that is
//! there, and never under a run. Behind `quality:RunPipeline` on
//! `pipeline/<dataset>` (a dataset's chain of extraction and evaluation),
//! asked of the request's [`Caller`], and audited as a run started by hand
//! (0026).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use chrono::Utc;
use contracts::{AuditEntry, Caller, Change};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

use crate::catalog::Catalog;
use crate::firing::{open_cut, CutRequest, Outcome};
use crate::mapping::Mapping;
use crate::schedule::{self, Rerun, DEFAULT_TIMEZONE};

pub const RUN: &str = "quality:RunPipeline";

#[derive(Clone)]
struct CutApi {
    pool: PgPool,
    catalog: Option<Arc<dyn Catalog>>,
}

/// `POST /quality/datasets/{dataset}/cuts`.
pub fn routes(pool: PgPool, catalog: Option<Arc<dyn Catalog>>) -> Router {
    Router::new()
        .route("/quality/datasets/{dataset}/cuts", post(run_now))
        .with_state(CutApi { pool, catalog })
}

/// The route's OpenAPI path and schemas.
#[derive(Debug, OpenApi)]
#[openapi(
    paths(run_now),
    components(schemas(RunBody, RunOut)),
    tags((name = "cuts", description = "A dataset's cuts: extraction, then evaluation")),
)]
pub struct CutDoc;

/// What to do when today's cut is already there.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct RunBody {
    /// `block` (the default) or `replace`: a cut still extracting or
    /// evaluating is never replaced.
    #[serde(default)]
    #[schema(value_type = String, example = "block")]
    pub rerun: Rerun,
}

/// What _Run now_ did.
#[derive(Debug, Serialize, ToSchema)]
pub struct RunOut {
    /// `queued` (a new cut), `replaced` or `blocked`.
    pub outcome: String,
    /// The cut's date, `YYYY-MM-DD`.
    pub cut: String,
    #[schema(value_type = Option<String>)]
    pub cut_id: Option<Uuid>,
    #[schema(value_type = Option<String>)]
    pub extract_job: Option<Uuid>,
}

fn status(code: StatusCode, error: &str) -> Response {
    (code, Json(json!({ "error": error }))).into_response()
}

/// Opens today's cut of `dataset` with its extraction.
#[utoipa::path(
    post,
    path = "/quality/datasets/{dataset}/cuts",
    operation_id = "quality_run_now",
    params(("dataset" = String, Path, description = "The dataset, as the catalog names it")),
    request_body = RunBody,
    responses(
        (status = 201, description = "Today's cut opened, or replaced, with its extraction",
            body = RunOut),
        (status = 401, description = "No valid session"),
        (status = 403, description = "May not run this dataset"),
        (status = 404, description = "No such dataset in the catalog"),
        (status = 409, description = "Today's cut is there and is not replaced", body = RunOut),
        (status = 422, description = "`no_source`: the dataset is read from no source"),
        (status = 503, description = "No catalog to know the dataset"),
    ),
    tag = "cuts",
)]
async fn run_now(
    State(api): State<CutApi>,
    Extension(caller): Extension<Caller>,
    Path(dataset): Path<String>,
    body: Option<Json<RunBody>>,
) -> Response {
    let resource = format!("pipeline/{dataset}");
    if !caller.can(RUN, &resource) {
        return status(StatusCode::FORBIDDEN, "forbidden");
    }
    if let Err((code, why)) = source_of(&api, &dataset).await {
        return status(code, why);
    }
    let now = Utc::now();
    let Ok(date) = schedule::cut_date(now, DEFAULT_TIMEZONE) else {
        return status(StatusCode::INTERNAL_SERVER_ERROR, "timezone");
    };
    let request = CutRequest {
        dataset: &dataset,
        date,
        rerun: body.map(|b| b.0.rerun).unwrap_or_default(),
        schedule: None,
        requested_by: Uuid::parse_str(&caller.account).ok(),
        due_at: now,
    };
    let opened = async {
        let mut tx = api.pool.begin().await?;
        let opened = open_cut(&mut tx, &request).await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(opened)
    };
    let (outcome, cut_id, extract_job) = match opened.await {
        Ok(o) => o,
        Err(e) => {
            tracing::error!("run now: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let out = RunOut {
        outcome: outcome.as_str().into(),
        cut: date.to_string(),
        cut_id,
        extract_job,
    };
    if outcome == Outcome::Blocked {
        return (StatusCode::CONFLICT, Json(out)).into_response();
    }
    let entry = AuditEntry {
        action: RUN,
        resource,
        change: Change::Ran,
    };
    match caller.audit(entry).await {
        Ok(()) => (StatusCode::CREATED, Json(out)).into_response(),
        Err(e) => {
            tracing::error!("run now: audit: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// The source `dataset`'s mapping on `main` reads (its `from`), or the
/// answer that refuses the run: a dataset no source feeds (loaded into the
/// lake) has nothing to extract, its cut would only fail.
async fn source_of(api: &CutApi, dataset: &str) -> Result<String, (StatusCode, &'static str)> {
    let unavailable = (StatusCode::SERVICE_UNAVAILABLE, "catalog_unavailable");
    let none = (
        StatusCode::SERVICE_UNAVAILABLE,
        crate::refusal::NOT_CONFIGURED,
    );
    let catalog = api.catalog.as_ref().ok_or(none)?;
    match catalog.snapshot(dataset).await {
        Ok(Some(snap)) => Mapping::parse(&snap.mapping)
            .ok()
            .and_then(|m| m.from)
            .map(|from| from.value.source)
            .ok_or((StatusCode::UNPROCESSABLE_ENTITY, "no_source")),
        Ok(None) => Err((StatusCode::NOT_FOUND, "no_such_dataset")),
        Err(e) => {
            tracing::error!("run now: catalog: {e}");
            Err(unavailable)
        }
    }
}
