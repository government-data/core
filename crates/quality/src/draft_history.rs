//! A draft read as the branch it is (decision 0021): its commits, newest
//! first, from where it left `main`; and what a commit changed, or the
//! whole draft against where it left `main`, each file before and after.
//! And a column's history on `main` (decision 0050): the commits that
//! changed its rules file, or the dataset's mapping, each before and after.
//! Behind `quality:ViewRules` on `catalog/<dataset>`; nothing here writes.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router};
use contracts::Caller;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, OpenApi, ToSchema};

use crate::draft_api::{self, DraftCatalog, Refused, VIEW};

/// One commit of a draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct DraftCommit {
    pub commit: String,
    /// The commit it follows (the first follows `main`'s).
    pub parent: String,
    /// `Name <email>`.
    pub author: String,
    pub at: String,
    pub message: String,
}

/// A draft's commits, newest first, and the commit of `main` it left from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct DraftHistory {
    pub base: String,
    pub commits: Vec<DraftCommit>,
}

/// A file before and after; `null` where it was not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct FileChange {
    /// A rules file's name, or `mapping` for the dataset's mapping.
    pub file: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// A change on `main` to one file: the commit that made it, and the file
/// before and after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct MainChange {
    pub commit: DraftCommit,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// The changes on `main` a column's history shows at most.
pub const HISTORY_LIMIT: usize = 50;

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct FileQuery {
    /// A rules file of the dataset; without it, its mapping.
    pub file: Option<String>,
}

/// What changed from `from` to `to`.
#[derive(Debug, Serialize, ToSchema)]
pub struct ChangesOut {
    pub from: String,
    pub to: String,
    pub files: Vec<FileChange>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ChangesQuery {
    /// One of the draft's commits; without it, the whole draft.
    pub commit: Option<String>,
}

/// `GET /quality/drafts/{dataset}/{id}/history` and `…/changes`.
pub fn routes(catalog: Arc<dyn DraftCatalog>) -> Router {
    Router::new()
        .route("/quality/drafts/{dataset}/{id}/history", get(history))
        .route("/quality/drafts/{dataset}/{id}/changes", get(changes))
        .route("/quality/datasets/{dataset}/history", get(file_history))
        .with_state(catalog)
}

/// The routes' OpenAPI paths and schemas.
#[derive(Debug, OpenApi)]
#[openapi(
    paths(history, changes, file_history),
    components(schemas(DraftHistory, DraftCommit, ChangesOut, FileChange, MainChange))
)]
pub struct HistoryDoc;

fn unavailable(e: String) -> Refused {
    tracing::error!("draft history: {e}");
    Refused(StatusCode::SERVICE_UNAVAILABLE, "catalog_unavailable")
}

/// The draft's branch and history, if the caller may read it.
async fn of(
    catalog: &dyn DraftCatalog,
    caller: &Caller,
    dataset: &str,
    id: &str,
) -> Result<(String, DraftHistory), Refused> {
    if !caller.can(VIEW, &format!("catalog/{dataset}")) {
        return Err(Refused(StatusCode::FORBIDDEN, "forbidden"));
    }
    let branch = draft_api::branch(dataset, id)?;
    let history = catalog
        .history(&branch)
        .await
        .map_err(unavailable)?
        .ok_or(Refused(StatusCode::NOT_FOUND, "not_found"))?;
    Ok((branch, history))
}

/// A draft's commits, newest first, and where it left `main`.
#[utoipa::path(get, path = "/quality/drafts/{dataset}/{id}/history", operation_id = "quality_draft_history",
    params(("dataset" = String, Path), ("id" = String, Path)),
    responses((status = 200, body = DraftHistory), (status = 401), (status = 403), (status = 404),
        (status = 503)), tag = "drafts")]
async fn history(
    State(catalog): State<Arc<dyn DraftCatalog>>,
    Extension(caller): Extension<Caller>,
    Path((dataset, id)): Path<(String, String)>,
) -> Result<Json<DraftHistory>, Refused> {
    let (_, history) = of(catalog.as_ref(), &caller, &dataset, &id).await?;
    Ok(Json(history))
}

/// What one of a draft's commits changed, or the whole draft against where
/// it left `main`: each file of the dataset that changed, before and after.
#[utoipa::path(get, path = "/quality/drafts/{dataset}/{id}/changes", operation_id = "quality_draft_changes",
    params(("dataset" = String, Path), ("id" = String, Path), ChangesQuery),
    responses((status = 200, body = ChangesOut), (status = 401), (status = 403),
        (status = 404, description = "No such draft, or the commit is not one of its"),
        (status = 503)), tag = "drafts")]
async fn changes(
    State(catalog): State<Arc<dyn DraftCatalog>>,
    Extension(caller): Extension<Caller>,
    Path((dataset, id)): Path<(String, String)>,
    Query(query): Query<ChangesQuery>,
) -> Result<Json<ChangesOut>, Refused> {
    let (branch, history) = of(catalog.as_ref(), &caller, &dataset, &id).await?;
    let (from, to) = match query.commit {
        None => (history.base, branch),
        Some(commit) => {
            let c = history
                .commits
                .into_iter()
                .find(|c| c.commit == commit)
                .ok_or(Refused(StatusCode::NOT_FOUND, "not_found"))?;
            (c.parent, c.commit)
        }
    };
    let files = catalog
        .changes(&from, &to, &dataset)
        .await
        .map_err(unavailable)?;
    Ok(Json(ChangesOut { from, to, files }))
}

/// A column's history on `main` (decision 0050): the changes to its rules
/// file, or to the dataset's mapping without `file`, newest first, at most
/// [`HISTORY_LIMIT`].
#[utoipa::path(get, path = "/quality/datasets/{dataset}/history", operation_id = "quality_file_history",
    params(("dataset" = String, Path), FileQuery),
    responses((status = 200, body = [MainChange]), (status = 401), (status = 403),
        (status = 422, description = "`file`: not a rules file's name"), (status = 503)),
    tag = "drafts")]
async fn file_history(
    State(catalog): State<Arc<dyn DraftCatalog>>,
    Extension(caller): Extension<Caller>,
    Path(dataset): Path<String>,
    Query(query): Query<FileQuery>,
) -> Result<Json<Vec<MainChange>>, Refused> {
    if !caller.can(VIEW, &format!("catalog/{dataset}")) {
        return Err(Refused(StatusCode::FORBIDDEN, "forbidden"));
    }
    if query
        .file
        .as_deref()
        .is_some_and(|f| !crate::edit_api::valid_file(f))
    {
        return Err(Refused(StatusCode::UNPROCESSABLE_ENTITY, "file"));
    }
    let changes = catalog
        .file_history(&dataset, query.file.as_deref(), HISTORY_LIMIT)
        .await
        .map_err(unavailable)?;
    Ok(Json(changes))
}
