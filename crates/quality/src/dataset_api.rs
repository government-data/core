//! What the cuts and schedules screen reads (decisions 0035 and 0040), in
//! the module's `user_router`: the datasets the catalog's `main` has, and a
//! dataset's recent cuts with their state, their run, where they came from,
//! the records evaluated and their manifest. Behind `quality:ViewPipelines`
//! on `pipeline/<dataset>`, asked of the request's [`Caller`]; the rules
//! screen asks for the datasets by `quality:ViewRules` on
//! `catalog/<dataset>` (decision 0019).

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use contracts::Caller;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

use crate::catalog::Catalog;
use crate::mapping::Mapping;
use crate::refusal::{refuse, refuse_because};

pub const VIEW: &str = "quality:ViewPipelines";
/// What the rules screen asks a dataset for (decision 0019).
const VIEW_RULES: &str = "quality:ViewRules";
/// The most recent cuts a list shows.
const RECENT: i64 = 60;

#[derive(Clone)]
struct DatasetApi {
    pool: PgPool,
    catalog: Option<Arc<dyn Catalog>>,
}

/// `GET /quality/datasets` and `GET /quality/datasets/{dataset}/cuts`.
pub fn routes(pool: PgPool, catalog: Option<Arc<dyn Catalog>>) -> Router {
    Router::new()
        .route("/quality/datasets", get(datasets))
        .route("/quality/datasets/{dataset}/cuts", get(cuts))
        .with_state(DatasetApi { pool, catalog })
}

/// The routes' OpenAPI paths and schemas.
#[derive(Debug, OpenApi)]
#[openapi(
    paths(datasets, cuts),
    components(schemas(DatasetOut, DatasetFrom, FieldColumn, CutOut))
)]
pub struct DatasetDoc;

/// A dataset of the catalog's `main`, and where its mapping reads it
/// (decision 0043, point 1): nothing of it if the mapping does not read.
#[derive(Debug, Serialize, ToSchema)]
pub struct DatasetOut {
    pub name: String,
    /// The source's table an extraction reads; none for a dataset loaded
    /// into the lake by hand.
    pub from: Option<DatasetFrom>,
    /// Its table in the lake's `<name>_bronze` namespace.
    pub bronze: Option<String>,
    /// Each field of the mapping and the column it reads.
    pub fields: Vec<FieldColumn>,
    /// Its latest cut's date (`YYYY-MM-DD`); none before its first run
    /// (decision 0049: _no cuts yet_).
    pub last_cut: Option<String>,
}

/// A source's table: the source by name, its schema if the mapping names
/// one, and the table.
#[derive(Debug, Serialize, ToSchema)]
pub struct DatasetFrom {
    pub source: String,
    pub schema: Option<String>,
    pub table: String,
}

/// A field of a mapping and the column it reads.
#[derive(Debug, Serialize, ToSchema)]
pub struct FieldColumn {
    pub field: String,
    pub column: String,
}

impl DatasetOut {
    /// A dataset by its mapping's text; only its name if it does not read.
    fn read(name: String, yaml: &str) -> Self {
        let Ok(m) = Mapping::parse(yaml) else {
            return Self {
                name,
                from: None,
                bronze: None,
                fields: Vec::new(),
                last_cut: None,
            };
        };
        let from = m.from.map(|f| {
            let (schema, table) = f.value.schema_and_table();
            DatasetFrom {
                source: f.value.source.clone(),
                schema: schema.map(str::to_string),
                table: table.to_string(),
            }
        });
        let fields = m
            .fields
            .into_iter()
            .map(|(field, f)| FieldColumn {
                field,
                column: f.value.column,
            })
            .collect();
        Self {
            name,
            from,
            bronze: Some(m.source.value),
            fields,
            last_cut: None,
        }
    }
}

/// A dataset's cut, as the screen shows it.
#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct CutOut {
    #[schema(value_type = String)]
    pub id: Uuid,
    /// `YYYY-MM-DD`.
    pub cut: String,
    /// `extracting`, `evaluating`, `done` or `failed`.
    pub state: String,
    /// 1, and one more each time it was replaced.
    pub run: i32,
    /// Fired by a schedule; else asked by a person (_Run now_).
    pub scheduled: bool,
    #[schema(value_type = String, format = DateTime)]
    pub due_at: DateTime<Utc>,
    #[schema(value_type = String, format = DateTime)]
    pub fired_at: DateTime<Utc>,
    /// The records its evaluation read, once evaluated.
    pub records: Option<i64>,
    /// Its evaluation's manifest in the warehouse bucket, once evaluated.
    pub manifest: Option<String>,
    /// Failed: at `extract` or `evaluate`, and what its job said (the
    /// worker's words, which name no value).
    pub failed_at: Option<String>,
    pub error: Option<String>,
}

/// Which screen asks: `pipelines` (the default) or `rules`.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DatasetsQuery {
    #[serde(rename = "for")]
    #[param(rename = "for")]
    pub for_: Option<String>,
}

/// The datasets whose pipeline the caller may see, by name; with
/// `for=rules`, those whose rules (`catalog/<dataset>`).
#[utoipa::path(get, path = "/quality/datasets", operation_id = "quality_list_datasets", tag = "cuts",
    params(DatasetsQuery),
    responses((status = 200, body = [DatasetOut]), (status = 401), (status = 403),
        (status = 422, description = "`for` is neither `pipelines` nor `rules`"),
        (status = 503, description = "No catalog to know them")))]
async fn datasets(
    State(api): State<DatasetApi>,
    Extension(caller): Extension<Caller>,
    Query(query): Query<DatasetsQuery>,
) -> Response {
    let (action, kind) = match query.for_.as_deref() {
        None | Some("pipelines") => (VIEW, "pipeline"),
        Some("rules") => (VIEW_RULES, "catalog"),
        Some(_) => return refuse(StatusCode::UNPROCESSABLE_ENTITY, "for"),
    };
    if !caller.can_some(action) {
        return refuse(StatusCode::FORBIDDEN, "forbidden");
    }
    let Some(catalog) = &api.catalog else {
        return crate::refusal::no_catalog();
    };
    let last: Result<Vec<(String, String)>, sqlx::Error> =
        sqlx::query_as("SELECT dataset, max(cut)::text FROM quality.cuts GROUP BY dataset")
            .fetch_all(&api.pool)
            .await;
    let Ok(last) = last.map(|rows| rows.into_iter().collect::<HashMap<_, _>>()) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    match catalog.mappings().await {
        Ok(mappings) => {
            let seen: Vec<DatasetOut> = mappings
                .into_iter()
                .filter(|m| caller.can(action, &format!("{kind}/{}", m.dataset)))
                .map(|m| DatasetOut {
                    last_cut: last.get(&m.dataset).cloned(),
                    ..DatasetOut::read(m.dataset, &m.yaml)
                })
                .collect();
            Json(seen).into_response()
        }
        Err(e) => {
            tracing::error!("datasets: catalog: {e}");
            let (code, cause) = ("catalog_unavailable", "catalog_unreachable");
            refuse_because(StatusCode::SERVICE_UNAVAILABLE, code, cause)
        }
    }
}

/// A dataset's most recent cuts, newest first.
#[utoipa::path(get, path = "/quality/datasets/{dataset}/cuts", operation_id = "quality_list_cuts", tag = "cuts",
    params(("dataset" = String, Path)),
    responses((status = 200, body = [CutOut]), (status = 401), (status = 403)))]
async fn cuts(
    State(api): State<DatasetApi>,
    Extension(caller): Extension<Caller>,
    Path(dataset): Path<String>,
) -> Response {
    if !caller.can(VIEW, &format!("pipeline/{dataset}")) {
        return refuse(StatusCode::FORBIDDEN, "forbidden");
    }
    let rows = recent_cuts(&api.pool, &dataset, RECENT).await;
    match rows {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => {
            tracing::error!("cuts: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// A dataset's `limit` most recent cuts, newest first, each failed one with
/// its stage and why.
pub(crate) async fn recent_cuts(
    pool: &PgPool,
    dataset: &str,
    limit: i64,
) -> Result<Vec<CutOut>, sqlx::Error> {
    sqlx::query_as(
        "SELECT c.id, c.cut::text AS cut, c.state, c.run, c.schedule_id IS NOT NULL AS scheduled,
                c.due_at, c.fired_at, s.records, s.manifest,
                CASE WHEN c.state = 'failed' THEN
                     CASE WHEN e.state = 'failed' THEN 'extract' ELSE 'evaluate' END END AS failed_at,
                CASE WHEN c.state = 'failed' THEN COALESCE(v.error, e.error) END AS error
           FROM quality.cuts c
           LEFT JOIN quality.run_summary s
             ON s.dataset = c.dataset AND s.cut = c.cut
           LEFT JOIN quality.jobs e ON e.id = c.extract_job
           LEFT JOIN quality.jobs v ON v.id = c.evaluate_job
          WHERE c.dataset = $1
          ORDER BY c.cut DESC
          LIMIT $2",
    )
    .bind(dataset)
    .bind(limit)
    .fetch_all(pool)
    .await
}
