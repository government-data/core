//! A maintain job on the application's side (decisions 0039 and 0061): its
//! credentials reach its dataset's bronze and silver tables only, as the catalog says
//! where they are; its report, per table the files before and after, the
//! snapshots expired and the orphans deleted, leaves the run's manifest,
//! `manifests/<dataset>/maintain/<job>.json`. Names and counts only.

use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::job_sources;
use crate::lake::{self, LakeAccess};
use crate::layers;
use crate::manifest::{self, FORMAT};
use crate::worker_api::WorkerApi;

/// Snapshots are kept 13 months, 0017's hot retention (decision 0039).
pub const SNAPSHOT_DAYS: i64 = 396;
/// Nothing younger than this is expired or deleted, whatever a job asks.
pub const MIN_DAYS: i64 = 7;

/// The retention a maintain job runs with: its parameters', never under
/// [`MIN_DAYS`]; `(snapshot days, orphan days)`.
pub fn retention(parameters: &serde_json::Value) -> (i64, i64) {
    let days = |key: &str, default: i64| {
        parameters
            .get(key)
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(default)
            .max(MIN_DAYS)
    };
    (
        days("snapshot_days", SNAPSHOT_DAYS),
        days("orphan_days", MIN_DAYS),
    )
}

/// One table as a maintain job left it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableMaintained {
    /// Its namespace (`<dataset>_bronze` or `<dataset>_silver`); none in a
    /// report from before silver, all bronze.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub namespace: String,
    pub table: String,
    pub files_before: i64,
    pub files_after: i64,
    pub snapshots_expired: i64,
    pub orphans_deleted: i64,
}

/// What a maintain job reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaintainReport {
    pub snapshot_days: i64,
    pub orphan_days: i64,
    pub tables: Vec<TableMaintained>,
}

/// A maintain run's manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaintainManifest {
    pub format: u32,
    pub job: Uuid,
    pub dataset: String,
    pub reported_at: String,
    #[serde(flatten)]
    pub report: MaintainReport,
}

/// `manifests/<dataset>/maintain/<job>.json`.
pub fn maintain_key(dataset: &str, job: Uuid) -> String {
    format!("manifests/{dataset}/maintain/{job}.json")
}

/// The job's credentials for its dataset's bronze and silver tables, each
/// namespace recorded first.
pub(crate) async fn access(
    api: &WorkerApi,
    job: Uuid,
    client: &str,
    headers: &HeaderMap,
    dataset: &str,
) -> Result<Option<LakeAccess>, StatusCode> {
    let (Some(issuer), Some(catalog)) = (&api.services.lake, &api.services.tables) else {
        return Ok(None);
    };
    let warehouse = lake::WAREHOUSE.to_string();
    let unavailable = |e: String| {
        tracing::error!("maintain: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    };
    let mut tables = Vec::new();
    for namespace in layers::namespaces(dataset) {
        for table in catalog
            .tables(&warehouse, &namespace)
            .await
            .map_err(unavailable)?
        {
            if let Some(prefix) = catalog
                .table_prefix(&warehouse, &namespace, &table)
                .await
                .map_err(unavailable)?
            {
                tables.push((namespace.clone(), table, prefix));
            }
        }
        let resource = format!("warehouse/{warehouse}/{namespace}");
        job_sources::record(
            api.services.issues.as_deref(),
            job,
            "maintain",
            client,
            headers,
            &resource,
        )
        .await?;
    }
    let grant = lake::maintain_grant(&warehouse, &tables);
    issuer
        .issue(&format!("job-{job}"), &grant)
        .await
        .map(Some)
        .map_err(unavailable)
}

/// Writes the run's manifest from its report.
pub(crate) async fn report(
    api: &WorkerApi,
    id: Uuid,
    dataset: &str,
    body: serde_json::Value,
) -> Result<(), StatusCode> {
    let report: MaintainReport =
        serde_json::from_value(body).map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    let counts_hold = report.tables.iter().all(|t| {
        [
            t.files_before,
            t.files_after,
            t.snapshots_expired,
            t.orphans_deleted,
        ]
        .iter()
        .all(|n| *n >= 0)
    });
    if !counts_hold || report.snapshot_days < MIN_DAYS || report.orphan_days < MIN_DAYS {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let Some(store) = &api.services.manifests else {
        return Ok(());
    };
    let m = MaintainManifest {
        format: FORMAT,
        job: id,
        dataset: dataset.to_string(),
        reported_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        report,
    };
    manifest::write(store.as_ref(), maintain_key(dataset, id), &m)
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_jobs_retention_is_its_own_never_under_seven_days() {
        assert_eq!(retention(&serde_json::json!({})), (SNAPSHOT_DAYS, MIN_DAYS));
        assert_eq!(
            retention(&serde_json::json!({ "snapshot_days": 30, "orphan_days": 1 })),
            (30, MIN_DAYS)
        );
        assert_eq!(
            maintain_key("loans", Uuid::nil()),
            "manifests/loans/maintain/00000000-0000-0000-0000-000000000000.json"
        );
    }
}
