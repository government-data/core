//! An extraction's credentials for its bronze and silver tables (decisions
//! 0037 and 0061): to create each table in its dataset's namespace, and to
//! write its files once the catalog, never the worker, says where they are.
//! On a first extraction the worker creates the tables and asks again
//! (`GET /internal/jobs/{tenant}/{id}/lake`).

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use uuid::Uuid;

use crate::job_sources;
use crate::lake::{self, LakeAccess, TableCatalog};
use crate::layers::{bronze, silver};
use crate::mapping::Mapping;
use crate::worker_api::WorkerApi;

/// `namespace`, made if missing, and its table's prefix if it is there.
async fn place(
    tables: &dyn TableCatalog,
    warehouse: &str,
    (namespace, table): &(String, String),
) -> Result<Option<String>, StatusCode> {
    let unavailable = |e: String| {
        tracing::error!("{namespace}: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    };
    tables
        .ensure_namespace(warehouse, namespace)
        .await
        .map_err(unavailable)?;
    tables
        .table_prefix(warehouse, namespace, table)
        .await
        .map_err(unavailable)
}

/// An evaluation's, a rule test's or a console statement's credentials:
/// reading the warehouse, recorded first. None without an issuer.
pub(crate) async fn read_access(
    api: &WorkerApi,
    job: Uuid,
    kind: &str,
    client: &str,
    headers: &HeaderMap,
) -> Result<Option<LakeAccess>, StatusCode> {
    let Some(issuer) = &api.services.lake else {
        return Ok(None);
    };
    let warehouse = lake::WAREHOUSE.to_string();
    let resource = format!("warehouse/{warehouse}");
    let log = api.services.issues.as_deref();
    job_sources::record(log, job, kind, client, headers, &resource).await?;
    Ok(Some(job_sources::read_access(issuer.as_ref(), job).await?))
}

/// The extraction's credentials for `mapping`'s bronze and silver tables,
/// each recorded first.
pub(crate) async fn extract_access(
    api: &WorkerApi,
    job: Uuid,
    client: &str,
    headers: &HeaderMap,
    mapping: &str,
) -> Result<Option<LakeAccess>, StatusCode> {
    let (Some(issuer), Some(tables)) = (&api.services.lake, &api.services.tables) else {
        return Ok(None);
    };
    let Ok(mapping) = Mapping::parse(mapping) else {
        return Ok(None);
    };
    let warehouse = lake::WAREHOUSE.to_string();
    let mut placed = Vec::new();
    for at in std::iter::once(bronze(&mapping)).chain(silver(&mapping)) {
        let prefix = place(tables.as_ref(), &warehouse, &at).await?;
        let resource = format!("warehouse/{warehouse}/{}.{}", at.0, at.1);
        job_sources::record(
            api.services.issues.as_deref(),
            job,
            "extract",
            client,
            headers,
            &resource,
        )
        .await?;
        placed.push((at.0, prefix));
    }
    let placed: Vec<(&str, Option<&str>)> = placed
        .iter()
        .map(|(namespace, prefix)| (namespace.as_str(), prefix.as_deref()))
        .collect();
    let policy = lake::extract_policy(&warehouse, &placed);
    issuer
        .issue(&warehouse, &format!("job-{job}"), &policy)
        .await
        .map(Some)
        .map_err(|e| {
            tracing::error!("lake credentials: {e}");
            StatusCode::SERVICE_UNAVAILABLE
        })
}

/// `GET /internal/jobs/{tenant}/{id}/lake`: an extraction asks again, once
/// it has created its table.
pub(crate) async fn lake_again(
    State(api): State<WorkerApi>,
    Path((tenant, id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<LakeAccess>, StatusCode> {
    let client = api.caller(&headers)?;
    async {
        let job = api.held(tenant, id, &client).await?;
        let (Some(catalog), Some(dataset), "extract") =
            (&api.services.catalog, &job.dataset, job.kind.as_str())
        else {
            return Err(StatusCode::NOT_FOUND);
        };
        let snapshot = catalog
            .snapshot(dataset)
            .await
            .map_err(|e| {
                tracing::error!("catalog: {e}");
                StatusCode::SERVICE_UNAVAILABLE
            })?
            .ok_or(StatusCode::NOT_FOUND)?;
        extract_access(&api, id, &client, &headers, &snapshot.mapping)
            .await?
            .map(Json)
            .ok_or(StatusCode::SERVICE_UNAVAILABLE)
    }
    .await
}
