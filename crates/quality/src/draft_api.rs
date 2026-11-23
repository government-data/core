//! A tenant's drafts for people (decisions 0021 and 0038), in the module's
//! `user_router`: list the proposals, read one, approve it into `main` or
//! discard it. A draft is `proposal/<dataset>/<id>`. Reading needs
//! `quality:ViewRules` on `catalog/<dataset>`; approving, both
//! `quality:ApproveRules` there and `quality:ManageDatasets` on
//! `dataset/<dataset>` (a proposal has a mapping and rules); discarding,
//! the latter. A draft whose mapping does not read or whose rules do not
//! lint is not approved: `main` never gets one that breaks. Each approval
//! and discard is audited.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use contracts::{AuditEntry, Caller, Change};
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use utoipa::{OpenApi, ToSchema};

pub use crate::draft_catalog::{
    Approval, DraftCatalog, DraftFiles, DraftInfo, FileEdit, FileSaved, FilesEdit, MappingEdit,
};
use crate::mapping::Mapping;
use crate::source::valid_name;
use crate::source_store;
use quality_rules::lint::Severity;

pub const VIEW: &str = "quality:ViewRules";
pub const APPROVE: &str = "quality:ApproveRules";
pub const MANAGE: &str = "quality:ManageDatasets";
pub(crate) const PREFIX: &str = "proposal/";

#[derive(Clone)]
struct DraftApi {
    drafts: Arc<dyn DraftCatalog>,
    /// The sources' store: a draft that reads a source holds it while it is
    /// approved, so a deletion never slips in between (decision 0036).
    sources: Option<PgPool>,
}

/// `GET /quality/drafts`, `GET`/`DELETE /quality/drafts/{dataset}/{id}`,
/// `POST /quality/drafts/{dataset}/{id}/approve`.
pub fn routes(drafts: Arc<dyn DraftCatalog>, sources: Option<PgPool>) -> Router {
    Router::new()
        .route("/quality/drafts", get(list))
        .route(
            "/quality/drafts/{dataset}/{id}",
            get(read).merge(delete(discard)),
        )
        .route("/quality/drafts/{dataset}/{id}/approve", post(approve))
        .with_state(DraftApi { drafts, sources })
}

/// The routes' OpenAPI paths and schemas.
#[derive(Debug, OpenApi)]
#[openapi(
    paths(list, read, approve, discard),
    components(schemas(DraftInfo, DraftOut, RulesFileOut)),
    tags((name = "drafts", description = "Proposals of the catalog, to approve or discard")),
)]
pub struct DraftDoc;

/// A draft to review: its mapping and its rules files, each with the codes
/// of its lint errors.
#[derive(Debug, Serialize, ToSchema)]
pub struct DraftOut {
    pub branch: String,
    pub commit: String,
    pub mapping: String,
    /// Whether the mapping reads; if not, nothing else is checked.
    pub mapping_reads: bool,
    pub rules: Vec<RulesFileOut>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RulesFileOut {
    pub file: String,
    pub yaml: String,
    /// The codes of its lint errors: none to approve.
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Refused(pub StatusCode, pub &'static str);

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

fn unavailable(e: impl std::fmt::Display) -> Refused {
    tracing::error!("drafts: {e}");
    Refused(StatusCode::SERVICE_UNAVAILABLE, "catalog_unavailable")
}

/// The branch of `dataset`'s draft `id`, if both are names a branch may
/// have.
pub(crate) fn branch(dataset: &str, id: &str) -> Result<String, Refused> {
    let id_ok = !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if valid_name(dataset) && id_ok {
        Ok(format!("{PREFIX}{dataset}/{id}"))
    } else {
        Err(Refused(StatusCode::NOT_FOUND, "not_found"))
    }
}

fn review(branch: String, files: DraftFiles) -> DraftOut {
    let schema = Mapping::parse(&files.mapping).ok().map(|m| m.schema());
    // Each file with the draft's own global rules (decision 0045).
    let globals = crate::rules_api::global_rules(&files.rules).map(str::to_string);
    let rules = files
        .rules
        .into_iter()
        .map(|(file, yaml)| {
            let errors = schema.as_ref().map_or_else(Vec::new, |schema| {
                crate::rules_api::linted(&yaml, schema, Some(&file), globals.as_deref())
                    .findings
                    .iter()
                    .filter(|f| f.severity() == Severity::Error)
                    .map(|f| f.code.id().to_string())
                    .collect()
            });
            RulesFileOut { file, yaml, errors }
        })
        .collect();
    DraftOut {
        branch,
        commit: files.commit,
        mapping_reads: schema.is_some(),
        mapping: files.mapping,
        rules,
    }
}

/// The proposals the caller may see.
#[utoipa::path(get, path = "/quality/drafts", operation_id = "quality_list_drafts",
    responses((status = 200, body = [DraftInfo]), (status = 401), (status = 403)), tag = "drafts")]
async fn list(
    State(api): State<DraftApi>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Vec<DraftInfo>>, Refused> {
    if !caller.can_some(VIEW) {
        return Err(Refused(StatusCode::FORBIDDEN, "forbidden"));
    }
    let all = api.drafts.list(PREFIX).await.map_err(unavailable)?;
    let seen = all
        .into_iter()
        .filter(|d| {
            let dataset = d
                .branch
                .trim_start_matches(PREFIX)
                .split('/')
                .next()
                .unwrap_or_default();
            caller.can(VIEW, &format!("catalog/{dataset}"))
        })
        .collect();
    Ok(Json(seen))
}

/// A draft to review.
#[utoipa::path(get, path = "/quality/drafts/{dataset}/{id}", operation_id = "quality_read_draft",
    params(("dataset" = String, Path), ("id" = String, Path)),
    responses((status = 200, body = DraftOut), (status = 401), (status = 403), (status = 404)), tag = "drafts")]
async fn read(
    State(api): State<DraftApi>,
    Extension(caller): Extension<Caller>,
    Path((dataset, id)): Path<(String, String)>,
) -> Result<Json<DraftOut>, Refused> {
    if !caller.can(VIEW, &format!("catalog/{dataset}")) {
        return Err(Refused(StatusCode::FORBIDDEN, "forbidden"));
    }
    let branch = branch(&dataset, &id)?;
    let files = api
        .drafts
        .read(&branch, &dataset)
        .await
        .map_err(unavailable)?
        .ok_or(Refused(StatusCode::NOT_FOUND, "not_found"))?;
    Ok(Json(review(branch, files)))
}

/// Approves a draft into `main`, if it reads and lints.
#[utoipa::path(post, path = "/quality/drafts/{dataset}/{id}/approve", operation_id = "quality_approve_draft",
    params(("dataset" = String, Path), ("id" = String, Path)),
    responses(
        (status = 200, description = "Merged: `{commit}`"),
        (status = 401), (status = 403), (status = 404),
        (status = 409, description = "`stale` (main changed the dataset since) or `conflicts`"),
        (status = 422, description = "Its mapping does not read, its rules do not lint (with them), or the source it reads is not there (`unknown_source`)", body = DraftOut),
    ), tag = "drafts")]
async fn approve(
    State(api): State<DraftApi>,
    Extension(caller): Extension<Caller>,
    Path((dataset, id)): Path<(String, String)>,
) -> Result<Response, Refused> {
    if !caller.can(APPROVE, &format!("catalog/{dataset}"))
        || !caller.can(MANAGE, &format!("dataset/{dataset}"))
    {
        return Err(Refused(StatusCode::FORBIDDEN, "forbidden"));
    }
    let branch = branch(&dataset, &id)?;
    let files = api
        .drafts
        .read(&branch, &dataset)
        .await
        .map_err(unavailable)?
        .ok_or(Refused(StatusCode::NOT_FOUND, "not_found"))?;
    let reads = Mapping::parse(&files.mapping)
        .ok()
        .and_then(|m| m.from)
        .map(|f| f.value.source);
    let out = review(branch.clone(), files);
    if !out.mapping_reads || out.rules.iter().any(|r| !r.errors.is_empty()) {
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, Json(out)).into_response());
    }
    // The source it reads, held until the merge is done; one not there is
    // refused.
    let _held = match (&api.sources, reads) {
        (Some(pool), Some(source)) => match source_store::hold(pool, &source, false).await {
            Ok(Some(held)) => Some(held),
            Ok(None) => return Err(Refused(StatusCode::UNPROCESSABLE_ENTITY, "unknown_source")),
            Err(e) => return Err(unavailable(e)),
        },
        _ => None,
    };
    let author = format!("{0} <{0}@accounts.invalid>", caller.account);
    match api
        .drafts
        .approve(&branch, &dataset, &author)
        .await
        .map_err(unavailable)?
    {
        Approval::Merged(commit) => {
            let entry = AuditEntry {
                action: APPROVE,
                resource: format!("catalog/{dataset}/draft/{id}"),
                change: Change::Approved,
            };
            caller.audit(entry).await.map_err(unavailable)?;
            Ok(Json(json!({ "commit": commit })).into_response())
        }
        Approval::Stale => Err(Refused(StatusCode::CONFLICT, "stale")),
        Approval::Conflicts(tables) => Ok((
            StatusCode::CONFLICT,
            Json(json!({ "error": "conflicts", "tables": tables })),
        )
            .into_response()),
        Approval::NotFound => Err(Refused(StatusCode::NOT_FOUND, "not_found")),
    }
}

/// Discards a draft, unmerged.
#[utoipa::path(delete, path = "/quality/drafts/{dataset}/{id}", operation_id = "quality_discard_draft",
    params(("dataset" = String, Path), ("id" = String, Path)),
    responses((status = 204), (status = 401), (status = 403), (status = 404)), tag = "drafts")]
async fn discard(
    State(api): State<DraftApi>,
    Extension(caller): Extension<Caller>,
    Path((dataset, id)): Path<(String, String)>,
) -> Result<StatusCode, Refused> {
    if !caller.can(MANAGE, &format!("dataset/{dataset}")) {
        return Err(Refused(StatusCode::FORBIDDEN, "forbidden"));
    }
    let branch = branch(&dataset, &id)?;
    if !api.drafts.discard(&branch).await.map_err(unavailable)? {
        return Err(Refused(StatusCode::NOT_FOUND, "not_found"));
    }
    let entry = AuditEntry {
        action: MANAGE,
        resource: format!("catalog/{dataset}/draft/{id}"),
        change: Change::Deleted,
    };
    caller.audit(entry).await.map_err(unavailable)?;
    Ok(StatusCode::NO_CONTENT)
}
