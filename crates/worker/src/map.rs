//! The `map` job (decision 0038): one table of a source, read-only through
//! its driver with the connection locked down, its metadata and its first
//! rows' aggregates, reported as the table's profile; the application
//! turns it into a proposal. Counts only, never a value; an engine's
//! message may quote one, so only the stage it failed in is kept.

use quality::job_map::MapReport;
use quality::proposal::{Patterns, TableProfile};
use quality::trino_profile::{columns_statement, profile_statement, read_columns, read_profile};
use quality::worker_api::JobDetails;

use crate::evaluate::{failed, Failed};
use crate::extract::with_source;
use crate::trino_job::JobTrino;

/// The sample when the job names none (decision 0038).
const SAMPLE: u64 = 10_000;

/// The job's source, its table, and the sample it asks.
fn asked(job: &JobDetails) -> Result<(&quality::job_sources::SourceAccess, u64), Failed> {
    let source = job
        .source
        .as_ref()
        .ok_or_else(|| failed("map: the job came with no source"))?;
    let sample = job
        .parameters
        .get("sample")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(SAMPLE);
    Ok((source, sample))
}

/// Profiles the job's table through the source's catalog on Trino
/// (decision 0053): its columns, then its sample's aggregates.
pub(crate) async fn map_on_trino(
    job: &JobDetails,
    trino: &JobTrino<'_>,
) -> Result<MapReport, Failed> {
    let (source, sample) = asked(job)?;
    let engine =
        |stage: &'static str| move |_| failed(format!("map: the engine failed while {stage}"));
    let no_table = |_| failed(format!("map: the source has no table {}", source.table));
    let statement = columns_statement(&source.name, &source.table).map_err(no_table)?;
    let rows = trino
        .execute(&statement)
        .await
        .map_err(engine("reading the columns"))?;
    let columns = read_columns(&rows).map_err(no_table)?;
    let patterns = Patterns::of_job(&job.parameters);
    let statement = profile_statement(&source.name, &source.table, &columns, sample, &patterns)
        .map_err(no_table)?;
    let rows = trino
        .execute(&statement)
        .await
        .map_err(engine("profiling"))?;
    let (sampled, columns) =
        read_profile(columns, &rows).map_err(|_| failed("map: the engine answered no profile"))?;
    Ok(MapReport {
        profile: TableProfile {
            source: source.name.clone(),
            table: source.table.clone(),
            row_estimate: None,
            sampled,
            columns,
        },
    })
}

/// Profiles the job's table.
pub(crate) fn map(job: &JobDetails) -> Result<MapReport, Failed> {
    let (source, sample) = asked(job)?;
    let mut runner = with_source(job, "map")?;
    runner
        .lock_down()
        .map_err(|_| failed("map: the engine failed while locking down"))?;
    let (sampled, columns) = runner
        .profile_source(&source.table, sample, &Patterns::of_job(&job.parameters))
        .map_err(|_| {
            failed(format!(
                "map: the engine failed while profiling {}",
                source.table
            ))
        })?;
    Ok(MapReport {
        profile: TableProfile {
            source: source.name.clone(),
            table: source.table.clone(),
            row_estimate: None,
            sampled,
            columns,
        },
    })
}
