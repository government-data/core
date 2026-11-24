//! A source's tables for people (decision 0042), in the module's
//! `user_router`: `GET /quality/sources/{name}/tables` gives its last
//! listing and where the next one stands, behind `quality:ViewSources`;
//! `POST` queues a new one, behind `quality:ManageSources` on
//! `source/<name>`, audited. Saving a source queues one too
//! ([`list_after_save`]).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use contracts::{AuditEntry, Caller, Change};
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

use crate::source_api::{MANAGE, VIEW};
use crate::source_tables::{self, listable, ListingReason, ListingState, ListingView, SourceTable};

/// `GET` and `POST /quality/sources/{name}/tables`.
pub fn routes(pool: PgPool) -> Router {
    Router::new()
        .route("/quality/sources/{name}/tables", get(listing).post(refresh))
        .with_state(pool)
}

/// The routes' OpenAPI paths and schemas.
#[derive(Debug, OpenApi)]
#[openapi(
    paths(listing, refresh),
    components(schemas(ListingView, ListingState, ListingReason, SourceTable, ListingQueued)),
    tags((name = "sources", description = "The sources and the driver catalog")),
)]
pub struct TablesDoc;

/// The listing job queued.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListingQueued {
    #[schema(value_type = String)]
    pub job: Uuid,
}

fn refuse(code: StatusCode, error: &str) -> Response {
    (code, Json(json!({ "error": error }))).into_response()
}

fn internal(e: &sqlx::Error) -> Response {
    tracing::error!("source tables: {e}");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

async fn driver_of(pool: &PgPool, name: &str) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT driver FROM quality.sources WHERE name = $1")
        .bind(name)
        .fetch_optional(pool)
        .await
}

/// The source's last listing, and whether a newer one is coming.
#[utoipa::path(
    get,
    path = "/quality/sources/{name}/tables",
    operation_id = "quality_source_tables",
    params(("name" = String, Path, description = "The source's name")),
    responses(
        (status = 200, description = "Its last listing, names only", body = ListingView),
        (status = 401, description = "No valid session"),
        (status = 403, description = "May not view this source"),
        (status = 404, description = "No such source"),
    ),
    tag = "sources",
)]
async fn listing(
    State(pool): State<PgPool>,
    Extension(caller): Extension<Caller>,
    Path(name): Path<String>,
) -> Response {
    if !caller.can(VIEW, &format!("source/{name}")) {
        return refuse(StatusCode::FORBIDDEN, "forbidden");
    }
    match driver_of(&pool, &name).await {
        Ok(Some(_)) => {}
        Ok(None) => return refuse(StatusCode::NOT_FOUND, "no_such_source"),
        Err(e) => return internal(&e),
    }
    match source_tables::view(&pool, &name).await {
        Ok(view) => Json(view).into_response(),
        Err(e) => internal(&e),
    }
}

/// Lists the source's tables again.
#[utoipa::path(
    post,
    path = "/quality/sources/{name}/tables",
    operation_id = "quality_list_source_tables",
    params(("name" = String, Path, description = "The source's name")),
    responses(
        (status = 202, description = "The listing job queued", body = ListingQueued),
        (status = 401, description = "No valid session"),
        (status = 403, description = "May not manage this source"),
        (status = 404, description = "No such source"),
        (status = 422, description = "Its driver is not read by DuckDB yet (`engine`)"),
    ),
    tag = "sources",
)]
async fn refresh(
    State(pool): State<PgPool>,
    Extension(caller): Extension<Caller>,
    Path(name): Path<String>,
) -> Response {
    let resource = format!("source/{name}");
    if !caller.can(MANAGE, &resource) {
        return refuse(StatusCode::FORBIDDEN, "forbidden");
    }
    match driver_of(&pool, &name).await {
        Ok(Some(d)) if listable(&d) => {}
        Ok(Some(_)) => return refuse(StatusCode::UNPROCESSABLE_ENTITY, "engine"),
        Ok(None) => return refuse(StatusCode::NOT_FOUND, "no_such_source"),
        Err(e) => return internal(&e),
    }
    let job = match queue(&pool, &name).await {
        Ok(job) => job,
        Err(e) => return internal(&e),
    };
    let entry = AuditEntry {
        action: MANAGE,
        resource,
        change: Change::Ran,
    };
    if let Err(e) = caller.audit(entry).await {
        tracing::error!("source tables: audit: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    (StatusCode::ACCEPTED, Json(ListingQueued { job })).into_response()
}

async fn queue(pool: &PgPool, name: &str) -> Result<Uuid, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let job = source_tables::queue(&mut tx, name).await?;
    tx.commit().await?;
    Ok(job)
}

/// Queues the listing of a source just saved, if DuckDB reads it. The
/// source is saved either way: a listing that could not be queued is
/// logged, and can be asked again.
pub(crate) async fn list_after_save(pool: &PgPool, name: &str, driver: &str) {
    if !listable(driver) {
        return;
    }
    if let Err(e) = queue(pool, name).await {
        tracing::warn!(source = name, "listing after saving: {e}");
    }
}
