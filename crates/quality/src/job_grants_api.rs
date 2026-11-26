//! The job grants route (decision 0090, point 7.2; step 18): `engine-trino`'s
//! rules merge asks the core, with its machine token, for the job users'
//! neutral grants, filled from each dataset's mapping on the catalog's
//! `main` (the source it is extracted from) and its approved rules' references. Names only.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::{
    extract::State,
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde::Serialize;

use crate::catalog::Catalog;
use crate::job_grants::{job_grants, DatasetJobs, Grant};
use crate::mapping::Mapping;
use crate::worker_api::MachineTokens;

/// The audience the rules merge asks its machine token for.
pub const AUDIENCE: &str = "quality-grants";

/// What the route answers.
#[derive(Debug, Serialize)]
pub struct JobGrants {
    pub grants: Vec<Grant>,
}

#[derive(Clone)]
struct GrantsApi {
    catalog: Arc<dyn Catalog>,
    tokens: Arc<dyn MachineTokens>,
}

/// `GET /internal/quality/v1/job-grants`.
pub fn routes(catalog: Arc<dyn Catalog>, tokens: Arc<dyn MachineTokens>) -> Router {
    Router::new()
        .route("/internal/quality/v1/job-grants", get(grants_of))
        .with_state(GrantsApi { catalog, tokens })
}

fn refused(status: StatusCode, class: &str) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(serde_json::json!({ "error": class })))
}

async fn grants_of(
    State(api): State<GrantsApi>,
    headers: HeaderMap,
) -> Result<Json<JobGrants>, (StatusCode, Json<serde_json::Value>)> {
    headers
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .and_then(|t| api.tokens.client(t))
        .ok_or_else(|| refused(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    let unavailable = |e: String| {
        tracing::error!("job grants: the catalog: {e}");
        refused(StatusCode::SERVICE_UNAVAILABLE, "catalog_unavailable")
    };
    let mut datasets = Vec::new();
    for m in api.catalog.mappings().await.map_err(unavailable)? {
        let Ok(mapping) = Mapping::parse(&m.yaml) else {
            tracing::warn!(dataset = m.dataset, "job grants: a mapping does not parse");
            continue;
        };
        let snapshot = api
            .catalog
            .snapshot(&m.dataset)
            .await
            .map_err(unavailable)?;
        let references: BTreeSet<String> = snapshot
            .iter()
            .flat_map(|s| s.rules.iter())
            .filter_map(|r| quality_rules::parse(&r.yaml).ok())
            .flat_map(|f| f.references.into_values().map(|r| r.dataset.value))
            .filter(|d| *d != m.dataset)
            .collect();
        datasets.push(DatasetJobs {
            dataset: m.dataset,
            sources: mapping.from.map(|f| f.value.source).into_iter().collect(),
            references: references.into_iter().collect(),
        });
    }
    Ok(Json(JobGrants {
        grants: job_grants(&datasets),
    }))
}
