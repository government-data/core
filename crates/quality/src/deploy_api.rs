//! A column's rules file deployed to `main` (decision 0051), in the
//! module's `user_router`: linted as a save is, refused if `main`'s file
//! changed since the person opened it (`stale`), then one commit on `main`
//! authored by them. Reverting is deploying an earlier version from the
//! column's history. Behind `quality:ApproveRules` on `catalog/<dataset>`
//! and `quality:ManageDatasets` on `dataset/<dataset>`, as approving a
//! draft is; each deploy audited.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::put;
use axum::{Extension, Json, Router};
use contracts::{AuditEntry, Caller, Change};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{OpenApi, ToSchema};

use crate::draft_api::{DraftCatalog, FileEdit, FileSaved, Refused, APPROVE, MANAGE};
use crate::edit_api::{check, valid_file};
use crate::refusal::refuse;
use crate::rules_api::{self, MAIN};
use crate::source::valid_name;

/// `PUT /quality/datasets/{dataset}/files/{file}`.
pub fn routes(catalog: Arc<dyn DraftCatalog>) -> Router {
    Router::new()
        .route("/quality/datasets/{dataset}/files/{file}", put(deploy))
        .with_state(catalog)
}

/// The routes' OpenAPI paths and schemas.
#[derive(Debug, OpenApi)]
#[openapi(paths(deploy), components(schemas(DeployBody, DeployedOut)))]
pub struct DeployDoc;

/// A column's file to deploy, and the file as the person opened it.
#[derive(Debug, Deserialize, ToSchema)]
pub struct DeployBody {
    pub yaml: String,
    /// `main`'s file when it was opened; `null` if it had none.
    pub expected: Option<String>,
}

/// `main`'s commit after the deploy.
#[derive(Debug, Serialize, ToSchema)]
pub struct DeployedOut {
    pub commit: String,
}

/// Deploys a column's rules file to `main`.
#[utoipa::path(put, path = "/quality/datasets/{dataset}/files/{file}", operation_id = "quality_deploy_file",
    tag = "rules", params(("dataset" = String, Path), ("file" = String, Path)),
    request_body = DeployBody,
    responses((status = 200, body = DeployedOut), (status = 401), (status = 403),
        (status = 404, description = "No such dataset, or a file it may not add"),
        (status = 409, description = "`stale`: `main`'s file changed since it was opened"),
        (status = 422, description = "`lint`: the errors that block it, as `findings`"),
        (status = 503)))]
async fn deploy(
    State(catalog): State<Arc<dyn DraftCatalog>>,
    Extension(caller): Extension<Caller>,
    Path((dataset, file)): Path<(String, String)>,
    Json(body): Json<DeployBody>,
) -> Response {
    if !valid_name(&dataset)
        || !valid_file(&file)
        || !caller.can(APPROVE, &format!("catalog/{dataset}"))
        || !caller.can(MANAGE, &format!("dataset/{dataset}"))
    {
        return refuse(StatusCode::FORBIDDEN, "forbidden");
    }
    let checked = match check(catalog.as_ref(), &dataset, (&file, None), &body.yaml).await {
        Ok(checked) if checked.errors.is_empty() => checked,
        Ok(checked) => {
            let out = json!({ "error": "lint", "findings": checked.errors });
            return (StatusCode::UNPROCESSABLE_ENTITY, Json(out)).into_response();
        }
        Err(Refused(code, error)) => return refuse(code, error),
    };
    let now = match rules_api::on_main(catalog.as_ref(), &dataset).await {
        Ok(Some(files)) => files
            .rules
            .into_iter()
            .find(|(f, _)| *f == file)
            .map(|(_, b)| b),
        Ok(None) => return refuse(StatusCode::NOT_FOUND, "not_found"),
        Err((code, error)) => return refuse(code, error),
    };
    if now != body.expected {
        return refuse(StatusCode::CONFLICT, "stale");
    }
    let author = format!("{0} <{0}@accounts.invalid>", caller.account);
    let edit = FileEdit {
        branch: MAIN,
        dataset: &dataset,
        file: &file,
        body: &body.yaml,
        author: &author,
        adds: checked.adds,
    };
    let commit = match catalog.save_file(&edit, false).await {
        Ok(FileSaved::Saved(commit)) => commit,
        Ok(FileSaved::NoDraft | FileSaved::NoFile) => {
            return refuse(StatusCode::NOT_FOUND, "not_found")
        }
        Err(e) => {
            tracing::error!("deploy: catalog: {e}");
            return refuse(
                StatusCode::SERVICE_UNAVAILABLE,
                crate::refusal::catalog_code(&e),
            );
        }
    };
    let entry = AuditEntry {
        action: APPROVE,
        resource: format!("catalog/{dataset}/file/{file}"),
        change: Change::Changed,
    };
    if let Err(e) = caller.audit(entry).await {
        tracing::error!("deploy: audit: {e}");
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "audit_unavailable");
    }
    (StatusCode::OK, Json(DeployedOut { commit })).into_response()
}
