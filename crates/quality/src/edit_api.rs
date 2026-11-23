//! A person's edit of a rules file, saved as a draft (decisions 0019 and
//! 0021), in the module's `user_router`: the first save makes a draft
//! `proposal/<dataset>/<id>` from `main` with that file changed; the next
//! ones save on it, one commit each, the person its author. A file whose
//! lint finds errors is not saved (0019, point 8): the findings come back.
//! A file `main` lacks is added only when it is a field's own,
//! `<field>.yaml` (decision 0044), or the table's global rules (0045).
//! A draft is one column's (decision 0050): once it changes one rules
//! file, a save of another is refused (`other_file`).
//! Behind `quality:EditRules` on `catalog/<dataset>`; each save audited.
//! The draft is approved, or discarded, from the drafts' routes.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{post, put};
use axum::{Extension, Json, Router};
use contracts::{AuditEntry, Caller, Change};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

use crate::draft_api::{self, DraftCatalog, FileEdit, FileSaved, Refused};
use crate::mapping::Mapping;
use crate::rules_api::{self, FindingOut};
use crate::source::valid_name;
use quality_rules::model::GLOBAL_FILE;

pub const EDIT: &str = "quality:EditRules";

/// `POST /quality/datasets/{dataset}/drafts` and
/// `PUT /quality/drafts/{dataset}/{id}/files/{file}`.
pub fn routes(catalog: Arc<dyn DraftCatalog>) -> Router {
    Router::new()
        .route("/quality/datasets/{dataset}/drafts", post(create))
        .route("/quality/drafts/{dataset}/{id}/files/{file}", put(save))
        .with_state(catalog)
}

/// The routes' OpenAPI paths and schemas.
#[derive(Debug, OpenApi)]
#[openapi(
    paths(create, save),
    components(schemas(NewDraftBody, FileBody, SavedOut))
)]
pub struct EditDoc;

/// A rules file of `main`, edited: the draft it starts.
#[derive(Debug, Deserialize, ToSchema)]
pub struct NewDraftBody {
    pub file: String,
    pub yaml: String,
}

/// A rules file's new text.
#[derive(Debug, Deserialize, ToSchema)]
pub struct FileBody {
    pub yaml: String,
}

/// The draft saved on.
#[derive(Debug, Serialize, ToSchema)]
pub struct SavedOut {
    pub id: String,
    pub branch: String,
    pub commit: String,
}

/// A rules file's name: letters, digits, `_` and `-`, then `.yaml`.
pub(crate) fn valid_file(file: &str) -> bool {
    file.strip_suffix(".yaml").is_some_and(|stem| {
        !stem.is_empty()
            && stem.len() <= 64
            && stem
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

fn refuse(code: StatusCode, error: &str) -> Response {
    (code, Json(json!({ "error": error }))).into_response()
}

/// What `main` says of a file to save: the findings that block saving
/// `yaml`, by `dataset`'s mapping, and whether `file` may be added.
pub(crate) struct Checked {
    pub errors: Vec<FindingOut>,
    pub adds: bool,
    /// The mapping's fields.
    pub fields: Vec<String>,
}

pub(crate) async fn check(
    catalog: &dyn DraftCatalog,
    dataset: &str,
    (file, branch): (&str, Option<&str>),
    yaml: &str,
) -> Result<Checked, Refused> {
    let files = match rules_api::on_main(catalog, dataset).await {
        Ok(Some(files)) => files,
        Ok(None) => return Err(Refused(StatusCode::NOT_FOUND, "not_found")),
        Err((code, error)) => return Err(Refused(code, error)),
    };
    let Ok(mapping) = Mapping::parse(&files.mapping) else {
        return Err(Refused(StatusCode::UNPROCESSABLE_ENTITY, "mapping"));
    };
    // The draft's global rules, if it is there; `main`'s otherwise (0045).
    let globals = rules_api::draft_globals(catalog, dataset, branch, &files.rules).await;
    let errors = rules_api::view(yaml, &mapping.schema(), Some(file), globals.as_deref())
        .findings
        .into_iter()
        .filter(|f| f.severity == "error")
        .collect();
    let bodies = files.rules.iter().map(|(_, body)| body.as_str());
    let adds =
        file == GLOBAL_FILE || fields_file(file, &mapping, std::iter::once(yaml).chain(bodies));
    Ok(Checked {
        errors,
        adds,
        fields: mapping.fields.keys().cloned().collect(),
    })
}

/// Whether `file` is a field's own file (decision 0044): `<field>.yaml` of
/// a field of the mapping, or of one derived in any of `bodies`.
fn fields_file<'a>(file: &str, mapping: &Mapping, bodies: impl Iterator<Item = &'a str>) -> bool {
    let Some(field) = file.strip_suffix(".yaml") else {
        return false;
    };
    mapping.fields.contains_key(field)
        || bodies
            .filter_map(|body| quality_rules::parse(body).ok())
            .any(|rules| rules.derived.contains_key(field))
}

/// The one rules file a draft changes, if that is all it changes: a
/// column's draft (decision 0050); `None` for a draft of the dataset or
/// one not there.
async fn draft_column(
    catalog: &dyn DraftCatalog,
    branch: &str,
    dataset: &str,
) -> Result<Option<String>, String> {
    let Some(history) = catalog.history(branch).await? else {
        return Ok(None);
    };
    let changes = catalog.changes(&history.base, branch, dataset).await?;
    Ok(match changes.as_slice() {
        [one] if one.file != "mapping" => Some(one.file.clone()),
        _ => None,
    })
}

/// Saves `yaml` as `file` on draft `id` of `dataset`, audited.
async fn save_on(
    catalog: &dyn DraftCatalog,
    caller: &Caller,
    at: (&str, &str, &str),
    yaml: &str,
    create: bool,
) -> Response {
    let (dataset, id, file) = at;
    if !valid_name(dataset) || !valid_file(file) || !caller.can(EDIT, &format!("catalog/{dataset}"))
    {
        return refuse(StatusCode::FORBIDDEN, "forbidden");
    }
    let branch = match draft_api::branch(dataset, id) {
        Ok(branch) => branch,
        Err(Refused(code, error)) => return refuse(code, error),
    };
    let on = (!create).then_some(branch.as_str());
    let adds = match check(catalog, dataset, (file, on), yaml).await {
        Ok(checked) if checked.errors.is_empty() => checked.adds,
        Ok(checked) => {
            let body = json!({ "error": "lint", "findings": checked.errors });
            return (StatusCode::UNPROCESSABLE_ENTITY, Json(body)).into_response();
        }
        Err(Refused(code, error)) => return refuse(code, error),
    };
    if !create {
        match draft_column(catalog, &branch, dataset).await {
            Ok(Some(column)) if column != file => {
                return refuse(StatusCode::CONFLICT, "other_file");
            }
            Ok(_) => {}
            Err(e) => {
                tracing::error!("edit: catalog: {e}");
                return refuse(StatusCode::SERVICE_UNAVAILABLE, "catalog_unavailable");
            }
        }
    }
    let author = format!("{0} <{0}@accounts.invalid>", caller.account);
    let edit = FileEdit {
        branch: &branch,
        dataset,
        file,
        body: yaml,
        author: &author,
        adds,
    };
    let commit = match catalog.save_file(&edit, create).await {
        Ok(FileSaved::Saved(commit)) => commit,
        Ok(FileSaved::NoDraft | FileSaved::NoFile) => {
            return refuse(StatusCode::NOT_FOUND, "not_found")
        }
        Err(e) => {
            tracing::error!("edit: catalog: {e}");
            return refuse(StatusCode::SERVICE_UNAVAILABLE, "catalog_unavailable");
        }
    };
    let entry = AuditEntry {
        action: EDIT,
        resource: format!("catalog/{dataset}/draft/{id}"),
        change: if create {
            Change::Created
        } else {
            Change::Changed
        },
    };
    if let Err(e) = caller.audit(entry).await {
        tracing::error!("edit: audit: {e}");
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "audit_unavailable");
    }
    let status = if create {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    let out = SavedOut {
        id: id.to_string(),
        branch,
        commit,
    };
    (status, Json(out)).into_response()
}

/// Starts a draft from `main` with one rules file edited.
#[utoipa::path(post, path = "/quality/datasets/{dataset}/drafts", operation_id = "quality_edit_rules",
    tag = "rules", params(("dataset" = String, Path)), request_body = NewDraftBody,
    responses((status = 201, body = SavedOut), (status = 401), (status = 403),
        (status = 404, description = "No such dataset or rules file on `main`"),
        (status = 422, description = "`lint`: the errors that block saving, as `findings`"),
        (status = 503)))]
async fn create(
    State(catalog): State<Arc<dyn DraftCatalog>>,
    Extension(caller): Extension<Caller>,
    Path(dataset): Path<String>,
    Json(body): Json<NewDraftBody>,
) -> Response {
    let id = Uuid::new_v4().to_string();
    save_on(
        catalog.as_ref(),
        &caller,
        (&dataset, &id, &body.file),
        &body.yaml,
        true,
    )
    .await
}

/// Saves a rules file on a draft, one more commit.
#[utoipa::path(put, path = "/quality/drafts/{dataset}/{id}/files/{file}", operation_id = "quality_save_draft_file",
    tag = "rules", params(("dataset" = String, Path), ("id" = String, Path), ("file" = String, Path)),
    request_body = FileBody,
    responses((status = 200, body = SavedOut), (status = 401), (status = 403),
        (status = 404, description = "No such draft or rules file"),
        (status = 409, description = "`other_file`: the draft is another column's (decision 0050)"),
        (status = 422, description = "`lint`: the errors that block saving, as `findings`"),
        (status = 503)))]
async fn save(
    State(catalog): State<Arc<dyn DraftCatalog>>,
    Extension(caller): Extension<Caller>,
    Path((dataset, id, file)): Path<(String, String, String)>,
    Json(body): Json<FileBody>,
) -> Response {
    save_on(
        catalog.as_ref(),
        &caller,
        (&dataset, &id, &file),
        &body.yaml,
        false,
    )
    .await
}
