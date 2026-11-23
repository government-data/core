//! Deleting a dataset or a table of the lake (decisions 0055 and 0061).
//! `DELETE /quality/datasets/{dataset}` deletes a dataset whole: its bronze
//! and silver tables from the lake with their files, its schedules, cuts
//! and results, and its mapping and rules from `main` (one commit; the
//! catalog's history keeps them). `DELETE /quality/lake/tables/{namespace}/{table}` deletes a
//! table of the lake no dataset reads, and
//! `DELETE /quality/lake/namespaces/{namespace}` all of a namespace's
//! tables, the namespace with the last. Each asks the name typed again
//! (`confirm`), is refused while a cut of the dataset runs, and is audited.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::delete;
use axum::{Extension, Json, Router};
use contracts::{AuditEntry, Caller, Change};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use utoipa::{OpenApi, ToSchema};

use crate::catalog::Catalog;
use crate::draft_catalog::DraftCatalog;
use crate::lake::{self, TableCatalog};
use crate::layers;
use crate::mapping::Mapping;

/// Who may delete: a dataset's manager; every dataset's, for a loose table.
pub const MANAGE: &str = "quality:ManageDatasets";

#[derive(Clone)]
struct DeleteApi {
    pool: PgPool,
    drafts: Arc<dyn DraftCatalog>,
    catalog: Option<Arc<dyn Catalog>>,
    lake: Option<Arc<dyn TableCatalog>>,
}

/// `DELETE /quality/datasets/{dataset}` and `…/lake/tables/{namespace}/{table}`.
pub fn routes(
    pool: PgPool,
    drafts: Arc<dyn DraftCatalog>,
    catalog: Option<Arc<dyn Catalog>>,
    lake: Option<Arc<dyn TableCatalog>>,
) -> Router {
    Router::new()
        .route("/quality/datasets/{dataset}", delete(delete_dataset))
        .route(
            "/quality/lake/tables/{namespace}/{table}",
            delete(delete_table),
        )
        .route(
            "/quality/lake/namespaces/{namespace}",
            delete(delete_namespace),
        )
        .with_state(DeleteApi {
            pool,
            drafts,
            catalog,
            lake,
        })
}

/// The routes' OpenAPI paths and schemas.
#[derive(Debug, OpenApi)]
#[openapi(
    paths(delete_dataset, delete_table, delete_namespace),
    components(schemas(Confirm))
)]
pub struct DeleteDoc;

/// The name of what is deleted, typed again.
#[derive(Debug, Deserialize, ToSchema)]
pub struct Confirm {
    pub confirm: String,
}

fn refuse(code: StatusCode, error: &str) -> Response {
    (code, Json(json!({ "error": error }))).into_response()
}

/// Why a deletion stopped: its status and its word.
type Refusal = (StatusCode, &'static str);

/// The lake's table, deleted with its files; a lake there is none of is
/// nothing to delete.
async fn drop_from_lake(api: &DeleteApi, namespace: &str, table: &str) -> Result<bool, Refusal> {
    let Some(lake) = &api.lake else {
        return Ok(false);
    };
    let dropped = lake
        .drop_table(lake::WAREHOUSE, namespace, table)
        .await
        .map_err(|e| {
            tracing::error!("delete: lake: {e}");
            (StatusCode::SERVICE_UNAVAILABLE, "lake_unavailable")
        })?;
    // The lake's last listing, without it.
    let forgotten = sqlx::query(
        "UPDATE quality.lake_tables SET tables = coalesce((SELECT jsonb_agg(t) FROM jsonb_array_elements(tables) t
           WHERE NOT (t->>'schema' = $1 AND t->>'name' = $2)), '[]'::jsonb)",
    )
    .bind(namespace)
    .bind(table)
    .execute(&api.pool)
    .await;
    if let Err(e) = forgotten {
        tracing::error!("delete: lake listing: {e}");
    }
    Ok(dropped)
}

async fn audit(caller: &Caller, resource: String) -> Result<(), Refusal> {
    let entry = AuditEntry {
        action: MANAGE,
        resource,
        change: Change::Deleted,
    };
    caller.audit(entry).await.map_err(|e| {
        tracing::error!("delete: audit: {e}");
        (StatusCode::SERVICE_UNAVAILABLE, "audit_unavailable")
    })
}

/// Deletes a dataset whole.
#[utoipa::path(delete, path = "/quality/datasets/{dataset}", operation_id = "quality_delete_dataset",
    tag = "cuts", params(("dataset" = String, Path)), request_body = Confirm,
    responses((status = 204, description = "Deleted"), (status = 401), (status = 403),
        (status = 404, description = "No such dataset on `main`"),
        (status = 409, description = "`running`: one of its cuts is extracting or evaluating"),
        (status = 422, description = "`confirm`: not its name"), (status = 503)))]
async fn delete_dataset(
    State(api): State<DeleteApi>,
    Extension(caller): Extension<Caller>,
    Path(dataset): Path<String>,
    Json(body): Json<Confirm>,
) -> Response {
    if !caller.can(MANAGE, &format!("dataset/{dataset}")) {
        return refuse(StatusCode::FORBIDDEN, "forbidden");
    }
    if body.confirm != dataset {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, "confirm");
    }
    let Some(catalog) = &api.catalog else {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "catalog_unavailable");
    };
    let mapping = match catalog.snapshot(&dataset).await {
        Ok(Some(snap)) => snap.mapping,
        Ok(None) => return refuse(StatusCode::NOT_FOUND, "not_found"),
        Err(e) => {
            tracing::error!("delete: catalog: {e}");
            return refuse(StatusCode::SERVICE_UNAVAILABLE, "catalog_unavailable");
        }
    };
    let running: Result<bool, _> = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM quality.cuts WHERE dataset = $1 AND state IN ('extracting', 'evaluating'))",
    )
    .bind(&dataset)
    .fetch_one(&api.pool)
    .await;
    match running {
        Ok(false) => {}
        Ok(true) => return refuse(StatusCode::CONFLICT, "running"),
        Err(e) => {
            tracing::error!("delete: {e}");
            return refuse(StatusCode::SERVICE_UNAVAILABLE, "unavailable");
        }
    }
    // The lake first: what is left of a deletion stopped halfway is found
    // and deleted again (decision 0055, point 5).
    // Its bronze and silver tables (decision 0061).
    if let Ok(table) = Mapping::parse(&mapping).map(|m| m.source.value) {
        for namespace in layers::namespaces(&dataset) {
            if let Err((code, why)) = drop_from_lake(&api, &namespace, &table).await {
                return refuse(code, why);
            }
        }
    }
    let rows = async {
        let mut tx = api.pool.begin().await?;
        for sql in [
            "DELETE FROM quality.run_summary WHERE dataset = $1",
            "DELETE FROM quality.cuts WHERE dataset = $1",
            "DELETE FROM quality.schedules WHERE dataset = $1",
        ] {
            sqlx::query(sql).bind(&dataset).execute(&mut *tx).await?;
        }
        tx.commit().await
    };
    if let Err(e) = rows.await {
        tracing::error!("delete: {e}");
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "unavailable");
    }
    let author = format!("{0} <{0}@accounts.invalid>", caller.account);
    if let Err(e) = api.drafts.remove_dataset(&dataset, &author).await {
        tracing::error!("delete: catalog: {e}");
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "catalog_unavailable");
    }
    match audit(&caller, format!("dataset/{dataset}")).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err((code, why)) => refuse(code, why),
    }
}

/// Deletes a table of the lake no dataset reads.
#[utoipa::path(delete, path = "/quality/lake/tables/{namespace}/{table}", operation_id = "quality_delete_lake_table",
    tag = "cuts", params(("namespace" = String, Path), ("table" = String, Path)), request_body = Confirm,
    responses((status = 204, description = "Deleted"), (status = 401), (status = 403),
        (status = 404, description = "No such table in the lake"),
        (status = 409, description = "`in_use`: a dataset reads it; delete the dataset"),
        (status = 422, description = "`confirm`: not `namespace.table`"), (status = 503)))]
async fn delete_table(
    State(api): State<DeleteApi>,
    Extension(caller): Extension<Caller>,
    Path((namespace, table)): Path<(String, String)>,
    Json(body): Json<Confirm>,
) -> Response {
    if !caller.can(MANAGE, "dataset/*") {
        return refuse(StatusCode::FORBIDDEN, "forbidden");
    }
    if body.confirm != format!("{namespace}.{table}") {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, "confirm");
    }
    let Some(catalog) = &api.catalog else {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "catalog_unavailable");
    };
    let mappings = match catalog.mappings().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("delete: catalog: {e}");
            return refuse(StatusCode::SERVICE_UNAVAILABLE, "catalog_unavailable");
        }
    };
    if read_by_dataset(&mappings, &namespace, &table) {
        return refuse(StatusCode::CONFLICT, "in_use");
    }
    match drop_from_lake(&api, &namespace, &table).await {
        Ok(true) => {}
        Ok(false) => return refuse(StatusCode::NOT_FOUND, "not_found"),
        Err((code, why)) => return refuse(code, why),
    }
    match audit(&caller, format!("lake/{namespace}.{table}")).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err((code, why)) => refuse(code, why),
    }
}

/// Whether a dataset reads `namespace.table` (its bronze or its silver
/// table).
fn read_by_dataset(
    mappings: &[crate::catalog::DatasetMapping],
    namespace: &str,
    table: &str,
) -> bool {
    mappings.iter().any(|m| {
        layers::namespaces(&m.dataset)
            .iter()
            .any(|n| n == namespace)
            && Mapping::parse(&m.yaml).is_ok_and(|x| x.source.value == table)
    })
}

/// Deletes every table of a namespace of the lake, and the namespace with
/// the last; none if a dataset reads one of them.
#[utoipa::path(delete, path = "/quality/lake/namespaces/{namespace}", operation_id = "quality_delete_lake_namespace",
    tag = "cuts", params(("namespace" = String, Path)), request_body = Confirm,
    responses((status = 204, description = "Deleted"), (status = 401), (status = 403),
        (status = 404, description = "No table in that namespace"),
        (status = 409, description = "`in_use`: a dataset reads one of its tables; delete the dataset"),
        (status = 422, description = "`confirm`: not the namespace"), (status = 503)))]
async fn delete_namespace(
    State(api): State<DeleteApi>,
    Extension(caller): Extension<Caller>,
    Path(namespace): Path<String>,
    Json(body): Json<Confirm>,
) -> Response {
    if !caller.can(MANAGE, "dataset/*") {
        return refuse(StatusCode::FORBIDDEN, "forbidden");
    }
    if body.confirm != namespace {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, "confirm");
    }
    let (Some(catalog), Some(lake)) = (&api.catalog, &api.lake) else {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "catalog_unavailable");
    };
    let tables = match lake.tables(lake::WAREHOUSE, &namespace).await {
        Ok(t) if t.is_empty() => return refuse(StatusCode::NOT_FOUND, "not_found"),
        Ok(t) => t,
        Err(e) => {
            tracing::error!("delete: lake: {e}");
            return refuse(StatusCode::SERVICE_UNAVAILABLE, "lake_unavailable");
        }
    };
    let mappings = match catalog.mappings().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("delete: catalog: {e}");
            return refuse(StatusCode::SERVICE_UNAVAILABLE, "catalog_unavailable");
        }
    };
    if tables
        .iter()
        .any(|t| read_by_dataset(&mappings, &namespace, t))
    {
        return refuse(StatusCode::CONFLICT, "in_use");
    }
    for table in &tables {
        if let Err((code, why)) = drop_from_lake(&api, &namespace, table).await {
            return refuse(code, why);
        }
    }
    match audit(&caller, format!("lake/{namespace}")).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err((code, why)) => refuse(code, why),
    }
}
