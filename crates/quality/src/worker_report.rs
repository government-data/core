//! The report a worker sends when its job is done (decisions 0018 and
//! 0023), on `POST /internal/jobs/{tenant}/{id}/report`: kept as its
//! kind's, inside the job's tenant, if the caller's worker holds the job.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::console_api;
use crate::cuts::{self, ExtractReport, Published};
use crate::job_maintain;
use crate::job_map;
use crate::lake_tables;
use crate::manifest::{self, EvaluateManifest, ExtractManifest};
use crate::records_api;
use crate::results;
use crate::rule_test;
use crate::source_tables;
use crate::worker_api::{internal, Report, WorkerApi};

/// A report the tables refuse (a bad date, negative or impossible counts,
/// a rule twice) is the worker's error; anything else is ours.
fn rejected(e: sqlx::Error) -> StatusCode {
    let invalid = matches!(&e, sqlx::Error::Database(d)
        if d.is_check_violation() || d.is_unique_violation()
            || d.code().is_some_and(|c| c.starts_with("22")));
    if invalid {
        StatusCode::UNPROCESSABLE_ENTITY
    } else {
        internal(e)
    }
}

pub(crate) async fn report(
    State(api): State<WorkerApi>,
    Path((tenant, id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<StatusCode, StatusCode> {
    let client = api.caller(&headers)?;
    async {
        let job = api.held(tenant, id, &client).await?;
        if job.kind == source_tables::KIND {
            return source_tables::report(&api.pool, id, &job.parameters, body).await;
        }
        if job.kind == lake_tables::KIND {
            return lake_tables::report(&api.pool, id, body).await;
        }
        if job.kind == records_api::KIND {
            return records_api::report(&api.pool, id, &job.parameters, body).await;
        }
        if job.kind == console_api::KIND {
            return console_api::report(&api.pool, id, body).await;
        }
        // Reports are a dataset's, of a cut that is a date.
        let dataset = job.dataset.ok_or(StatusCode::UNPROCESSABLE_ENTITY)?;
        match job.kind.as_str() {
            "evaluate" => {
                let report: Report = parse(body, |r: &Report| &r.cut)?;
                if !report.entities_hashed() {
                    return Err(StatusCode::UNPROCESSABLE_ENTITY);
                }
                let manifest = match &api.services.manifests {
                    Some(store) => {
                        let key = manifest::evaluate_key(&dataset, &report.cut, id);
                        let m = EvaluateManifest::new(id, &dataset, &report, now());
                        Some(manifest::write(store.as_ref(), tenant, key, &m).await?)
                    }
                    None => None,
                };
                results::store(
                    &api.pool,
                    tenant,
                    id,
                    &dataset,
                    &report,
                    manifest.as_deref(),
                )
                .await
                .map_err(rejected)?;
            }
            "maintain" => job_maintain::report(&api, tenant, id, &dataset, body).await?,
            "test_rule" => {
                let cut = job.parameters.get("cut").and_then(|c| c.as_str());
                rule_test::report(&api.pool, id, cut, body).await?;
            }
            "map" => {
                job_map::report(&api, tenant, &job.parameters, &dataset, body).await?;
            }
            "extract" => {
                let report: ExtractReport = parse(body, |r: &ExtractReport| &r.cut)?;
                // A cut's extraction reports its own date, before it leaves a file.
                let asked = job.parameters.get("cut").and_then(|c| c.as_str());
                if !report.is_valid() || asked.is_some_and(|c| c != report.cut) {
                    return Err(StatusCode::UNPROCESSABLE_ENTITY);
                }
                if let Some(store) = &api.services.manifests {
                    let key = manifest::extract_key(&dataset, &report.cut, id);
                    let m = ExtractManifest::new(id, &dataset, &report, now());
                    manifest::write(store.as_ref(), tenant, key, &m).await?;
                }
                let published = cuts::publish(&api.pool, id, &report)
                    .await
                    .map_err(rejected)?;
                if published == Published::WrongCut {
                    return Err(StatusCode::UNPROCESSABLE_ENTITY);
                }
            }
            _ => return Err(StatusCode::UNPROCESSABLE_ENTITY),
        }
        Ok(StatusCode::NO_CONTENT)
    }
    .await
}

/// A report of the job's kind, whose cut is a date.
fn parse<T: DeserializeOwned>(
    body: serde_json::Value,
    cut: impl Fn(&T) -> &String,
) -> Result<T, StatusCode> {
    let report: T = serde_json::from_value(body).map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    chrono::NaiveDate::parse_from_str(cut(&report), "%Y-%m-%d")
        .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    Ok(report)
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
