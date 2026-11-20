//! _Split by field_ (decisions 0044 and 0045), in the module's
//! `user_router`: a dataset's fields named as their columns (its mapping
//! and every rule that names them, those of the datasets its references
//! read too), then its rules files rewritten as its table's global rules
//! and a file per field, a rule over several fields written in each of
//! them and the patterns fields share named in the global rules, on a new
//! draft, in one commit, to be reviewed and approved as any other; `main`
//! is untouched. Behind `quality:EditRules` on `catalog/<dataset>`, and
//! `quality:ManageDatasets` on `dataset/<dataset>` when fields are
//! renamed; audited.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use contracts::{AuditEntry, Caller, Change};
use std::collections::BTreeMap;

use quality_rules::lint::{references, rename_fields, split_and_extract, SplitRefused};
use serde_json::json;
use utoipa::OpenApi;
use uuid::Uuid;

use crate::draft_api::{self, DraftCatalog, DraftFiles, FilesEdit, Refused, MANAGE};
use crate::edit_api::{SavedOut, EDIT};
use crate::mapping::Mapping;
use crate::mapping_rename::{renamed_mapping, renames, Renames};
use crate::rules_api;
use crate::source::valid_name;

/// `POST /quality/datasets/{dataset}/split`.
pub fn routes(catalog: Arc<dyn DraftCatalog>) -> Router {
    Router::new()
        .route("/quality/datasets/{dataset}/split", post(split))
        .with_state(catalog)
}

/// The route's OpenAPI path.
#[derive(Debug, OpenApi)]
#[openapi(paths(split))]
pub struct SplitDoc;

fn refuse(code: StatusCode, error: &str) -> Response {
    (code, Json(json!({ "error": error }))).into_response()
}

/// Why the files were not split, for the person: a reason and its values.
fn refused(why: SplitRefused) -> Response {
    let body = match why {
        SplitRefused::Errors { file } => {
            json!({ "error": "split", "reason": "errors", "file": file })
        }
        SplitRefused::Inline { file, key } => {
            json!({ "error": "split", "reason": "inline", "file": file, "key": key })
        }
        SplitRefused::Conflict { kind, name } => {
            json!({ "error": "split", "reason": "conflict", "kind": kind, "name": name })
        }
        SplitRefused::NoField { file, rule } => {
            json!({ "error": "split", "reason": "no_field", "file": file, "rule": rule })
        }
        SplitRefused::Lost => json!({ "error": "split", "reason": "lost" }),
    };
    (StatusCode::UNPROCESSABLE_ENTITY, Json(body)).into_response()
}

/// Splits a dataset's rules by field on a new draft.
#[utoipa::path(post, path = "/quality/datasets/{dataset}/split", operation_id = "quality_split_rules",
    tag = "rules", params(("dataset" = String, Path)),
    responses((status = 201, body = SavedOut), (status = 204, description = "Already split"),
        (status = 401), (status = 403), (status = 404),
        (status = 409, description = "`split` (`reason`: clash, `name`): two fields would take one column's name"),
        (status = 422, description = "`split`: why not (`reason`: errors, inline, conflict, no_field, lost)"),
        (status = 503)))]
async fn split(
    State(catalog): State<Arc<dyn DraftCatalog>>,
    Extension(caller): Extension<Caller>,
    Path(dataset): Path<String>,
) -> Response {
    if !valid_name(&dataset) || !caller.can(EDIT, &format!("catalog/{dataset}")) {
        return refuse(StatusCode::FORBIDDEN, "forbidden");
    }
    let files = match rules_api::on_main(catalog.as_ref(), &dataset).await {
        Ok(Some(files)) => files,
        Ok(None) => return refuse(StatusCode::NOT_FOUND, "not_found"),
        Err((code, error)) => return refuse(code, error),
    };
    let (changes, mapping) = match reorganized(catalog.as_ref(), &caller, &dataset, &files).await {
        Ok(Some(done)) => done,
        Ok(None) => return StatusCode::NO_CONTENT.into_response(),
        Err(refused) => return *refused,
    };
    let id = Uuid::new_v4().to_string();
    let branch = match draft_api::branch(&dataset, &id) {
        Ok(branch) => branch,
        Err(Refused(code, error)) => return refuse(code, error),
    };
    let Ok(tenant) = Uuid::parse_str(&tenancy::key()) else {
        return refuse(StatusCode::INTERNAL_SERVER_ERROR, "tenant");
    };
    let author = format!("{0} <{0}@accounts.invalid>", caller.account);
    let message = format!("catalog: split {dataset} by field");
    let edit = FilesEdit {
        branch: &branch,
        dataset: &dataset,
        files: &changes,
        mapping: mapping.as_deref(),
        author: &author,
        message: &message,
    };
    let commit = match catalog.save_files(tenant, &edit).await {
        Ok(commit) => commit,
        Err(e) => {
            tracing::error!("split: catalog: {e}");
            return refuse(StatusCode::SERVICE_UNAVAILABLE, "catalog_unavailable");
        }
    };
    let entry = AuditEntry {
        action: EDIT,
        resource: format!("catalog/{dataset}/draft/{id}"),
        change: Change::Created,
    };
    if let Err(e) = caller.audit(entry).await {
        tracing::error!("split: audit: {e}");
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "audit_unavailable");
    }
    let out = SavedOut { id, branch, commit };
    (StatusCode::CREATED, Json(out)).into_response()
}

/// The renames of each dataset `files`' references read, from its mapping
/// on `main`.
async fn others(
    catalog: &dyn DraftCatalog,
    files: &[(String, String)],
) -> BTreeMap<String, Renames> {
    let mut out = BTreeMap::new();
    for dataset in references(files).into_values() {
        if let Ok(Some(other)) = rules_api::on_main(catalog, &dataset).await {
            if let Some(r) = Mapping::parse(&other.mapping)
                .ok()
                .and_then(|m| renames(&m).ok())
            {
                out.insert(dataset, r);
            }
        }
    }
    out
}

/// What changes in `dataset`'s files on `main`: its rules files (new,
/// rewritten or removed) and its mapping if fields are renamed; `None` if
/// nothing does.
async fn reorganized(
    catalog: &dyn DraftCatalog,
    caller: &Caller,
    dataset: &str,
    files: &DraftFiles,
) -> Result<Option<(Vec<(String, Option<String>)>, Option<String>)>, Box<Response>> {
    let Ok(mapping) = Mapping::parse(&files.mapping) else {
        return Err(Box::new(refuse(
            StatusCode::UNPROCESSABLE_ENTITY,
            "mapping",
        )));
    };
    let own = renames(&mapping).map_err(|clash| {
        let body = json!({ "error": "split", "reason": "clash", "name": clash.name });
        Box::new((StatusCode::CONFLICT, Json(body)).into_response())
    })?;
    if !own.is_empty() && !caller.can(MANAGE, &format!("dataset/{dataset}")) {
        return Err(Box::new(refuse(StatusCode::FORBIDDEN, "manage")));
    }
    let others = others(catalog, &files.rules).await;
    let (rules, new_mapping, schema) = if own.is_empty() && others.values().all(BTreeMap::is_empty)
    {
        (files.rules.clone(), None, mapping.schema())
    } else {
        let text = renamed_mapping(&files.mapping, &own);
        let Ok(after) = Mapping::parse(&text) else {
            return Err(Box::new(refused(SplitRefused::Lost)));
        };
        let schema = after.schema();
        let rules = rename_fields(&files.rules, (&mapping.schema(), &schema), &own, &others)
            .ok_or_else(|| Box::new(refused(SplitRefused::Lost)))?;
        (rules, (text != files.mapping).then_some(text), schema)
    };
    let mut done = rules.clone();
    for (file, text) in split_and_extract(&rules, &schema).map_err(|why| Box::new(refused(why)))? {
        done.retain(|(f, _)| *f != file);
        if let Some(text) = text {
            done.push((file, text));
        }
    }
    // Each file new or rewritten, then each left out, against `main`.
    let mut changes: Vec<(String, Option<String>)> = done
        .iter()
        .filter(|(f, y)| files.rules.iter().all(|(g, z)| g != f || z != y))
        .map(|(f, y)| (f.clone(), Some(y.clone())))
        .collect();
    changes.extend(
        files
            .rules
            .iter()
            .filter(|(f, _)| done.iter().all(|(g, _)| g != f))
            .map(|(f, _)| (f.clone(), None)),
    );
    Ok((!changes.is_empty() || new_mapping.is_some()).then_some((changes, new_mapping)))
}
