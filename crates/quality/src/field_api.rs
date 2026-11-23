//! _Map the whole table_ (decisions 0043 and 0047), in the module's
//! `user_router`: the columns of a dataset's table that no field reads,
//! added to its mapping as fields, all at once, committed to `main` as the
//! person, so rules can be written for them right away; a table of the
//! lake no dataset reads starts the dataset, named by the explorer, with
//! all its columns. Only adds: no field there changes. Behind
//! `quality:ManageDatasets` on `dataset/<dataset>` (who may approve a
//! mapping's draft); audited.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use contracts::{AuditEntry, Caller, Change};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::{OpenApi, ToSchema};

use crate::draft_api::{DraftCatalog, FileSaved, MappingEdit, MANAGE};
use crate::mapping_field::{started, with_columns, NotAdded, Start};
use crate::rules_api;
use crate::source::valid_name;

/// `POST /quality/datasets/{dataset}/fields`.
pub fn routes(catalog: Arc<dyn DraftCatalog>) -> Router {
    Router::new()
        .route("/quality/datasets/{dataset}/fields", post(add))
        .with_state(catalog)
}

/// The route's OpenAPI path and schemas.
#[derive(Debug, OpenApi)]
#[openapi(
    paths(add),
    components(schemas(FieldsBody, ColumnBody, TableBody, FieldsOut))
)]
pub struct FieldDoc;

/// A table's columns to read as fields.
#[derive(Debug, Deserialize, ToSchema)]
pub struct FieldsBody {
    /// Every column of the table; those a field reads already are left.
    pub columns: Vec<ColumnBody>,
    /// The table, when no dataset reads it: the dataset is started
    /// reading it.
    pub table: Option<TableBody>,
}

/// A column of the table.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ColumnBody {
    /// The column, as the source names it.
    pub column: String,
    /// Its type in the source (`VARCHAR`, `INTEGER`, `DATE`…): the
    /// field's type in silver follows from it.
    #[serde(rename = "type")]
    pub kind: String,
}

/// A table no dataset reads: a source's, or the lake's (no source).
#[derive(Debug, Deserialize, ToSchema)]
pub struct TableBody {
    pub source: Option<String>,
    /// `schema.table` of a source; the table's name in the lake.
    pub table: String,
}

/// `main`'s commit with the fields, and the fields.
#[derive(Debug, Serialize, ToSchema)]
pub struct FieldsOut {
    pub commit: String,
    pub fields: Vec<String>,
}

/// A refusal: its status and body.
type Refusal = (StatusCode, Value);

fn refusal(code: StatusCode, error: &str) -> Refusal {
    (code, json!({ "error": error }))
}

fn valid_columns(columns: &[ColumnBody]) -> bool {
    let fine =
        |s: &str, max: usize| !s.is_empty() && s.len() <= max && !s.contains(char::is_control);
    !columns.is_empty()
        && columns.len() <= 1000
        && columns
            .iter()
            .all(|c| fine(&c.column, 128) && fine(&c.kind, 64))
}

/// The mapping with the columns added, the fields, and whether it starts
/// the dataset; or the refusal.
fn added(
    dataset: &str,
    yaml: Option<String>,
    body: &FieldsBody,
) -> Result<(String, Vec<String>, bool), Refusal> {
    let columns: Vec<(&str, &str)> = body
        .columns
        .iter()
        .map(|c| (c.column.as_str(), c.kind.as_str()))
        .collect();
    let refused = |(why, column): (NotAdded, String)| match why {
        NotAdded::Taken => (
            StatusCode::CONFLICT,
            json!({ "error": "field", "reason": "taken", "column": column }),
        ),
        _ => refusal(StatusCode::UNPROCESSABLE_ENTITY, "mapping"),
    };
    let (text, fields, starts) = match (yaml, &body.table) {
        // A table no dataset reads, but the name is one's.
        (Some(_), Some(_)) => return Err(refusal(StatusCode::CONFLICT, "dataset_taken")),
        (Some(yaml), None) => {
            let (text, fields) = with_columns(yaml, &columns).map_err(refused)?;
            (text, fields, false)
        }
        (None, Some(table)) => {
            let start = Start {
                source: table.source.as_deref(),
                table: &table.table,
            };
            let ((column, kind), rest) = columns.split_first().expect("columns checked");
            let (text, first) = started(dataset, start, column, kind)
                .map_err(|why| refused((why, column.to_string())))?;
            let (text, mut fields) = with_columns(text, rest).map_err(refused)?;
            fields.insert(0, first);
            (text, fields, true)
        }
        (None, None) => return Err(refusal(StatusCode::NOT_FOUND, "not_found")),
    };
    if fields.is_empty() {
        return Err(refusal(StatusCode::CONFLICT, "mapped"));
    }
    Ok((text, fields, starts))
}

/// Maps a table's columns no field reads, on `main`.
#[utoipa::path(post, path = "/quality/datasets/{dataset}/fields", operation_id = "quality_add_fields",
    tag = "rules", params(("dataset" = String, Path)), request_body = FieldsBody,
    responses((status = 201, body = FieldsOut), (status = 401), (status = 403),
        (status = 404, description = "No such dataset, and no table to start it from"),
        (status = 409, description = "`mapped`: every column is read already; `field` (`reason`: taken, `column`): a field of that name reads another column; `dataset_taken`: a table to start a dataset from, but the name is another's"),
        (status = 422, description = "`columns`: none, or not names; `mapping`: the mapping does not read, or has no `fields:` to add to"),
        (status = 503)))]
async fn add(
    State(catalog): State<Arc<dyn DraftCatalog>>,
    Extension(caller): Extension<Caller>,
    Path(dataset): Path<String>,
    Json(body): Json<FieldsBody>,
) -> Response {
    let refuse = |(code, out): Refusal| (code, Json(out)).into_response();
    if !valid_name(&dataset) || !caller.can(MANAGE, &format!("dataset/{dataset}")) {
        return refuse(refusal(StatusCode::FORBIDDEN, "forbidden"));
    }
    if !valid_columns(&body.columns) {
        return refuse(refusal(StatusCode::UNPROCESSABLE_ENTITY, "columns"));
    }
    let yaml = match rules_api::on_main(catalog.as_ref(), &dataset).await {
        Ok(files) => files.map(|f| f.mapping),
        Err((code, error)) => return refuse(refusal(code, error)),
    };
    let (text, fields, starts) = match added(&dataset, yaml, &body) {
        Ok(added) => added,
        Err(refused) => return refuse(refused),
    };
    let author = format!("{0} <{0}@accounts.invalid>", caller.account);
    let edit = MappingEdit {
        branch: "main",
        dataset: &dataset,
        body: &text,
        author: &author,
        starts,
    };
    let commit = match catalog.save_mapping(&edit, false).await {
        Ok(FileSaved::Saved(commit)) => commit,
        Ok(_) => return refuse(refusal(StatusCode::NOT_FOUND, "not_found")),
        Err(e) => {
            tracing::error!("fields: catalog: {e}");
            return refuse(refusal(
                StatusCode::SERVICE_UNAVAILABLE,
                "catalog_unavailable",
            ));
        }
    };
    let entry = AuditEntry {
        action: MANAGE,
        resource: format!("dataset/{dataset}"),
        change: if starts {
            Change::Created
        } else {
            Change::Changed
        },
    };
    if let Err(e) = caller.audit(entry).await {
        tracing::error!("fields: audit: {e}");
        return refuse(refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            "audit_unavailable",
        ));
    }
    let out = FieldsOut { commit, fields };
    (StatusCode::CREATED, Json(out)).into_response()
}
