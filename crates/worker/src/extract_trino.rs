//! The `extract` job of a source on Trino (decision 0048, point 3): the
//! statements of `quality::trino_extract`, run by Trino with the job's own
//! token (decision 0062), from the source's catalog into the cut of its bronze
//! table. Its user in Trino is its dataset, which Trino's rules may not
//! name yet just after it is mapped: a statement they refuse is tried once
//! more (`trino_job`). The worker sees no credential and no value: names, types and
//! counts come back. Trino's message may quote a name, so only the stage it
//! failed in is kept.

use std::time::Instant;

use quality::cuts::{ExtractReport, RangeRead};
use quality::mapping::Mapping;
use quality::trino_extract::{self, Extraction};
use quality::worker_api::JobDetails;
use serde_json::Value;

use crate::evaluate::{failed, Failed};
use crate::trino_job::JobTrino;

/// The job's Trino: each statement run at a named stage.
struct Run<'a, 'b> {
    trino: &'a JobTrino<'b>,
}

impl Run<'_, '_> {
    async fn at(&self, stage: &str, sql: &str) -> Result<Vec<Vec<Value>>, Failed> {
        self.trino
            .execute_when_allowed(sql)
            .await
            .map_err(|_| failed(format!("extract: the engine failed while {stage}")))
    }
}

/// The job's extraction, as statements.
fn extraction(job: &JobDetails) -> Result<Extraction, Failed> {
    let cut = job
        .parameters
        .get("cut")
        .and_then(Value::as_str)
        .ok_or_else(|| failed("extract: the job has no cut (parameters.cut, YYYY-MM-DD)"))?;
    let catalog = job
        .catalog
        .as_ref()
        .ok_or_else(|| failed("extract: the catalog has no mapping for the job's dataset"))?;
    let mapping = Mapping::parse(&catalog.mapping)
        .map_err(|_| failed("extract: the dataset's mapping does not parse"))?;
    let source = job
        .source
        .as_ref()
        .ok_or_else(|| failed("extract: the job came with no source"))?;
    Extraction::new(
        &source.name,
        &source.table,
        &mapping.dataset.value,
        &mapping.source.value,
        cut,
    )
    .map_err(|e| failed(format!("extract: the job cannot be written ({e:?})")))
}

/// Extracts the job's source table into its cut, through Trino.
pub(crate) async fn extract(
    job: &JobDetails,
    trino: &JobTrino<'_>,
) -> Result<ExtractReport, Failed> {
    let started = Instant::now();
    let plan = extraction(job)?;
    let run = Run { trino };
    let columns = trino_extract::source_columns(
        &run.at("reading the source's columns", &plan.columns())
            .await?,
    );
    if columns.is_empty() {
        return Err(failed(
            "extract: the source's table has no columns Trino sees",
        ));
    }
    let exists = trino_extract::number(
        &run.at("looking for the bronze table", &plan.exists())
            .await?,
    )
    .is_some_and(|n| n > 0);
    let written = if exists {
        let rows = run.at("counting the cut's rows", &plan.cut_rows()).await?;
        if trino_extract::number(&rows).is_some_and(|n| n > 0) {
            run.at("replacing the cut", &plan.delete()).await?;
        }
        run.at("replacing the cut", &plan.insert(&columns)).await?
    } else {
        run.at("creating the bronze table", &plan.create()).await?
    };
    let rows = trino_extract::number(&written)
        .ok_or_else(|| failed("extract: Trino said no row count"))?;
    let snapshot = trino_extract::text(&run.at("reading the snapshot", &plan.snapshot()).await?)
        .ok_or_else(|| failed("extract: the bronze table has no snapshot"))?;
    let source = job
        .source
        .as_ref()
        .map(|s| s.table.clone())
        .unwrap_or_default();
    Ok(ExtractReport {
        cut: job.parameters["cut"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        duration_ms: i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
        source,
        schema: columns,
        ranges: vec![RangeRead {
            range: "all".into(),
            rows,
        }],
        snapshot,
        // Not yet through Trino: its cuts are read from bronze (0061).
        silver_snapshot: None,
    })
}
