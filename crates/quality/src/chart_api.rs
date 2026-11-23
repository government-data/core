//! A dataset's charts (decision 0031), in the module's `user_router`: the
//! charts whose needs its mapping's field roles meet, with its evaluated
//! cuts, and one chart's
//! rows for a cut, run on the recent cuts PostgreSQL keeps. Only
//! aggregates: no value of a record. Behind `quality:ViewResults` on
//! `pipeline/<dataset>`, asked of the request's [`Caller`].

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use contracts::Caller;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use utoipa::{OpenApi, ToSchema};

use crate::catalog::Catalog;
use crate::charts::{self, Chart, Parameters};
use crate::mapping::Mapping;

pub const VIEW: &str = "quality:ViewResults";
/// The most recent evaluated cuts a dashboard offers.
const RECENT: i64 = 60;

/// `GET /quality/datasets/{dataset}/charts` and
/// `GET /quality/datasets/{dataset}/charts/{chart}`.
pub fn routes(pool: PgPool, catalog: Option<Arc<dyn Catalog>>) -> Router {
    Router::new()
        .route("/quality/datasets/{dataset}/charts", get(list))
        .route("/quality/datasets/{dataset}/charts/{chart}", get(rows))
        .with_state(ChartApi { pool, catalog })
}

#[derive(Clone)]
struct ChartApi {
    pool: PgPool,
    catalog: Option<Arc<dyn Catalog>>,
}

/// The routes' OpenAPI paths and schemas.
#[derive(Debug, OpenApi)]
#[openapi(paths(list, rows), components(schemas(ChartsOut, Chart, ChartRows)))]
pub struct ChartDoc;

/// A dataset's dashboard: its charts and the cuts they can show.
#[derive(Debug, Serialize, ToSchema)]
pub struct ChartsOut {
    pub charts: Vec<Chart>,
    /// Its evaluated cuts (`YYYY-MM-DD`), newest first.
    pub cuts: Vec<String>,
}

/// One chart's rows for a cut.
#[derive(Debug, Serialize, ToSchema)]
pub struct ChartRows {
    pub chart: String,
    /// The cut shown; none if the dataset has no evaluated cut.
    pub cut: Option<String>,
    /// One object per row, its columns as the chart's query names them.
    #[schema(value_type = Vec<Object>)]
    pub rows: Vec<Value>,
}

/// The cut to show: one evaluated; the latest if none asked.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CutQuery {
    /// `YYYY-MM-DD`.
    pub cut: Option<String>,
}

fn refuse(code: StatusCode, error: &str) -> Response {
    (code, Json(json!({ "error": error }))).into_response()
}

fn failed(what: &str, e: impl std::fmt::Display) -> Response {
    tracing::error!("charts: {what}: {e}");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

/// The charts a dataset's dashboard shows, and its evaluated cuts.
#[utoipa::path(get, path = "/quality/datasets/{dataset}/charts", operation_id = "quality_list_charts", tag = "charts",
    params(("dataset" = String, Path)),
    responses((status = 200, body = ChartsOut), (status = 401), (status = 403)))]
async fn list(
    State(api): State<ChartApi>,
    Extension(caller): Extension<Caller>,
    Path(dataset): Path<String>,
) -> Response {
    if !caller.can(VIEW, &format!("pipeline/{dataset}")) {
        return refuse(StatusCode::FORBIDDEN, "forbidden");
    }
    let roles = roles(&api, &dataset).await;
    let roles: Vec<&str> = roles.iter().map(String::as_str).collect();
    match evaluated(&api.pool, &dataset).await {
        Ok(cuts) => Json(ChartsOut {
            charts: charts::meeting(charts::CATALOG, &roles).copied().collect(),
            cuts,
        })
        .into_response(),
        Err(e) => failed("cuts", e),
    }
}

/// A chart's rows for an evaluated cut of the dataset.
#[utoipa::path(get, path = "/quality/datasets/{dataset}/charts/{chart}", operation_id = "quality_chart_rows", tag = "charts",
    params(("dataset" = String, Path), ("chart" = String, Path), CutQuery),
    responses((status = 200, body = ChartRows), (status = 401), (status = 403),
        (status = 404, description = "No such chart, or the cut was not evaluated")))]
async fn rows(
    State(api): State<ChartApi>,
    Extension(caller): Extension<Caller>,
    Path((dataset, chart)): Path<(String, String)>,
    Query(query): Query<CutQuery>,
) -> Response {
    if !caller.can(VIEW, &format!("pipeline/{dataset}")) {
        return refuse(StatusCode::FORBIDDEN, "forbidden");
    }
    let Some(chart) = charts::chart(&chart) else {
        return refuse(StatusCode::NOT_FOUND, "chart");
    };
    let cuts = match evaluated(&api.pool, &dataset).await {
        Ok(cuts) => cuts,
        Err(e) => return failed("cuts", e),
    };
    let at = match &query.cut {
        None => 0,
        Some(cut) => match cuts.iter().position(|c| c == cut) {
            Some(at) => at,
            None => return refuse(StatusCode::NOT_FOUND, "cut"),
        },
    };
    let Some(cut) = cuts.get(at) else {
        return Json(ChartRows {
            chart: chart.id.to_string(),
            cut: None,
            rows: Vec::new(),
        })
        .into_response();
    };
    let parameters = Parameters {
        dataset: &dataset,
        cut,
        previous_cut: cuts.get(at + 1).map(String::as_str),
    };
    match run(&api.pool, chart, &parameters).await {
        Ok(rows) => Json(ChartRows {
            chart: chart.id.to_string(),
            cut: Some(cut.clone()),
            rows,
        })
        .into_response(),
        Err(e) => failed(chart.id, e),
    }
}

/// The roles the dataset's mapping on the catalog's `main` gives its
/// fields, by name; none without a catalog or a mapping that reads.
async fn roles(api: &ChartApi, dataset: &str) -> Vec<String> {
    let Some(catalog) = &api.catalog else {
        return Vec::new();
    };
    match catalog.snapshot(dataset).await {
        Ok(Some(snapshot)) => Mapping::parse(&snapshot.mapping)
            .map(|m| {
                m.roles()
                    .into_iter()
                    .map(|r| r.name().to_string())
                    .collect()
            })
            .unwrap_or_default(),
        Ok(None) => Vec::new(),
        Err(e) => {
            tracing::warn!("charts: catalog: {e}");
            Vec::new()
        }
    }
}

/// The dataset's evaluated cuts, newest first: those with results.
async fn evaluated(pool: &PgPool, dataset: &str) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT cut::text FROM quality.run_summary WHERE dataset = $1 ORDER BY cut DESC LIMIT $2",
    )
    .bind(dataset)
    .bind(RECENT)
    .fetch_all(pool)
    .await
}

/// Runs a chart's query, each row as an object.
async fn run(
    pool: &PgPool,
    chart: &Chart,
    parameters: &Parameters<'_>,
) -> Result<Vec<Value>, String> {
    let (text, values) =
        charts::bind(chart.sql, parameters).map_err(|name| format!("unknown parameter :{name}"))?;
    let wrapped = format!("SELECT COALESCE(json_agg(t), '[]'::json) FROM ({text}) t");
    let mut query = sqlx::query_scalar::<_, Value>(&wrapped);
    for value in values {
        query = query.bind(value);
    }
    match query.fetch_one(pool).await.map_err(|e| e.to_string())? {
        Value::Array(rows) => Ok(rows),
        _ => Ok(Vec::new()),
    }
}
