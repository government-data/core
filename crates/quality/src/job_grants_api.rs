//! The job grants route (decision 0090, point 7.2; step 18): `engine-trino`'s
//! rules merge asks the core, with its machine token, for the job users'
//! neutral grants, filled from each dataset's mapping on the catalog's
//! `main` (the source it is extracted from) and its approved rules' references,
//! and, with the platform's IAM, the people's grants beside them (decision
//! 0089, step 5: [`crate::people_grants`]). Names and account ids only.

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
use contracts::Grantees;

use sqlx::PgPool;

use crate::job_grants::{console_grants, job_grants, ConsoleRun, DatasetJobs, Grant};
use crate::mapping::Mapping;
use crate::people_grants::{people_grants, PEOPLE_GOLD};
use crate::worker_api::MachineTokens;

/// The audience the rules merge asks its machine token for.
pub const AUDIENCE: &str = "core";

/// What the route answers.
#[derive(Debug, Serialize)]
pub struct JobGrants {
    pub grants: Vec<Grant>,
}

#[derive(Clone)]
struct GrantsApi {
    catalog: Arc<dyn Catalog>,
    tokens: Arc<dyn MachineTokens>,
    people: Option<Arc<dyn Grantees>>,
    /// Where the open console runs are read (ruling 84).
    consoles: Option<PgPool>,
}

/// `GET /internal/quality/v1/job-grants`, the job users' grants only.
pub fn routes(catalog: Arc<dyn Catalog>, tokens: Arc<dyn MachineTokens>) -> Router {
    serve(GrantsApi {
        catalog,
        tokens,
        people: None,
        consoles: None,
    })
}

/// The same, with the people's grants after the job users' (decision 0089,
/// step 5): each person's gold rows of the datasets they may view.
pub fn routes_with_people(
    catalog: Arc<dyn Catalog>,
    tokens: Arc<dyn MachineTokens>,
    people: Arc<dyn Grantees>,
) -> Router {
    serve(GrantsApi {
        catalog,
        tokens,
        people: Some(people),
        consoles: None,
    })
}

/// The same, with each open console run's user after the people's
/// (decision 0090, ruling 84): its asker's datasets, read from `pool`.
pub fn routes_with_consoles(
    catalog: Arc<dyn Catalog>,
    tokens: Arc<dyn MachineTokens>,
    people: Option<Arc<dyn Grantees>>,
    pool: PgPool,
) -> Router {
    serve(GrantsApi {
        catalog,
        tokens,
        people,
        consoles: Some(pool),
    })
}

/// The console runs queued or running within their lease's hour, with the
/// datasets their ask recorded.
async fn open_consoles(pool: &PgPool) -> Result<Vec<ConsoleRun>, sqlx::Error> {
    let rows: Vec<(uuid::Uuid, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT id, parameters->'datasets' FROM quality.jobs
          WHERE kind = 'query' AND state NOT IN ('done', 'failed')
            AND created_at > now() - interval '1 hour'",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, datasets)| ConsoleRun {
            target: format!("r{}", id.simple()),
            datasets: datasets
                .and_then(|d| serde_json::from_value(d).ok())
                .unwrap_or_default(),
        })
        .collect())
}

fn serve(api: GrantsApi) -> Router {
    Router::new()
        .route("/internal/quality/v1/job-grants", get(grants_of))
        .with_state(api)
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
            sources: mapping
                .from
                .as_ref()
                .map(|f| f.value.source.clone())
                .into_iter()
                .collect(),
            references: references.into_iter().collect(),
            table: Some(mapping.source.value),
        });
    }
    let mut grants = job_grants(&datasets);
    if let Some(people) = &api.people {
        let names: Vec<String> = datasets.iter().map(|d| d.dataset.clone()).collect();
        let theirs = people_grants(people.as_ref(), &names).await.map_err(|e| {
            tracing::error!("job grants: the people's: {e}");
            refused(StatusCode::SERVICE_UNAVAILABLE, "iam_unavailable")
        })?;
        grants.extend(theirs);
    }
    if let Some(pool) = &api.consoles {
        let runs = open_consoles(pool).await.map_err(|e| {
            tracing::error!("job grants: the console runs: {e}");
            refused(StatusCode::SERVICE_UNAVAILABLE, "unavailable")
        })?;
        grants.extend(console_grants(&runs, PEOPLE_GOLD));
    }
    Ok(Json(JobGrants { grants }))
}
