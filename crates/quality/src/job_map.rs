//! A map job on the application's side (decisions 0038 and 0053): its
//! report, the table's profile (counts, never a value), becomes a proposal
//! (`quality::proposal`) published on `main` of the tenant's catalog, and
//! today's cut of the new dataset is opened with its extraction. A repeated
//! report finds the dataset there and its cut opened.

use async_trait::async_trait;
use axum::http::StatusCode;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::firing::{open_cut, CutRequest};
use crate::proposal::{propose, Patterns, TableProfile};
use crate::schedule::{self, Rerun, DEFAULT_TIMEZONE};
use crate::worker_api::WorkerApi;

/// What a map job reports: the table it profiled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapReport {
    pub profile: TableProfile,
}

/// Where proposals are published: `main` of the tenant's catalog.
#[async_trait]
pub trait Drafts: Send + Sync {
    /// Writes the dataset on `main` if it is not there; its commit, or
    /// `None` if it is: a dataset is never written over by a map.
    async fn publish(
        &self,
        tenant: Uuid,
        dataset: &str,
        mapping: &str,
        rules: &[(String, String)],
    ) -> Result<Option<String>, String>;
}

/// Publishes a map job's report and opens the dataset's first cut.
pub(crate) async fn report(
    api: &WorkerApi,
    tenant: Uuid,
    parameters: &serde_json::Value,
    dataset: &str,
    body: serde_json::Value,
) -> Result<(), StatusCode> {
    let report: MapReport =
        serde_json::from_value(body).map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    // The table the job was asked for, of the source it was given.
    let asked = |key: &str| parameters.get(key).and_then(|v| v.as_str());
    if asked("source") != Some(report.profile.source.as_str())
        || asked("table") != Some(report.profile.table.as_str())
    {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let (Some(drafts), Some(alerts)) = (
        api.services.drafts.as_deref(),
        api.services.proposal_alerts.as_ref(),
    ) else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };
    // The patterns the job profiled with: its proposal checks the same.
    let proposal = propose(
        dataset,
        &report.profile,
        &Patterns::of_job(parameters),
        alerts,
    );
    drafts
        .publish(tenant, dataset, &proposal.mapping, &proposal.rules)
        .await
        .map_err(|e| {
            tracing::error!("map publish: {e}");
            StatusCode::SERVICE_UNAVAILABLE
        })?;
    first_cut(&api.pool, dataset, asked("requested_by"))
        .await
        .map_err(|e| {
            tracing::error!("map cut: {e}");
            StatusCode::SERVICE_UNAVAILABLE
        })
}

/// Today's cut of `dataset`, as _Run now_ opens it; one already there is
/// left as it is.
async fn first_cut(pool: &PgPool, dataset: &str, by: Option<&str>) -> Result<(), sqlx::Error> {
    let now = Utc::now();
    let date = schedule::cut_date(now, DEFAULT_TIMEZONE)
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    let request = CutRequest {
        dataset,
        date,
        rerun: Rerun::Block,
        schedule: None,
        requested_by: by.and_then(|b| Uuid::parse_str(b).ok()),
        due_at: now,
    };
    let mut tx = pool.begin().await?;
    open_cut(&mut tx, &request).await?;
    tx.commit().await
}
