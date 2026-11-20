//! The `extract` job (decisions 0036, 0037 and 0061): a dataset's source
//! table read whole, through its driver, into the cut of its bronze table,
//! and that cut typed into its silver table. In two connections: the first
//! reads the source's columns and, on a dataset's first extraction, creates
//! the tables, partitioned by `cut`; the second, with credentials for those
//! tables' files, replaces the cut in each. Names, types
//! and counts come out, never a value; an engine's message may quote one,
//! so only the stage it failed in is kept.

use std::time::Instant;

use quality::cuts::{ExtractReport, RangeRead, SourceColumn};
use quality::lake::LakeAccess;
use quality::layers;
use quality::mapping::Mapping;
use quality::source::driver;
use quality::worker_api::JobDetails;
use quality_rules_duckdb::{Column, LakeConnection, Runner, SourceConnection};

use crate::evaluate::{failed, Failed};

fn engine(stage: &str) -> impl Fn(quality_rules_duckdb::Error) -> Failed + '_ {
    move |_| failed(format!("extract: the engine failed while {stage}"))
}

/// What the first connection found: what the second writes.
pub(crate) struct Plan {
    cut: String,
    namespace: String,
    table: String,
    source_table: String,
    columns: Vec<Column>,
    mapping: Mapping,
    /// The silver table, unless the mapping cannot have one.
    silver: Option<(String, String)>,
    /// A table was created now: its files need credentials asked again.
    pub(crate) created: bool,
    started: Instant,
}

/// A runner with the job's source attached (its current credentials, then
/// the previous ones if refused) and the lake, locked down.
/// A runner with the job's source attached, read-only: its current
/// credentials, then the previous ones if refused (decision 0030). `stage`
/// names the job in its errors.
pub(crate) fn with_source(job: &JobDetails, stage: &str) -> Result<Runner, Failed> {
    let source = job
        .source
        .as_ref()
        .ok_or_else(|| failed(format!("{stage}: the job came with no source")))?;
    let default_port = driver(&source.driver).and_then(|d| d.default_port);
    let refused = || {
        failed(format!(
            "{stage}: the source {} refused the job",
            source.name
        ))
    };
    let mut runner = Runner::new().map_err(engine("starting"))?;
    let mut attach = |secrets| {
        runner.attach_source(&SourceConnection {
            driver: &source.driver,
            fields: &source.fields,
            secrets,
            default_port,
        })
    };
    if attach(&source.secrets).is_err() {
        let previous = source.previous.as_ref().ok_or_else(refused)?;
        attach(previous).map_err(|_| refused())?;
        tracing::warn!(source = %source.name, "the source took its previous credentials");
    }
    Ok(runner)
}

/// The job's source and the lake attached, locked down.
fn connect(job: &JobDetails, lake: &LakeAccess, ca_file: Option<&str>) -> Result<Runner, Failed> {
    let mut runner = with_source(job, "extract")?;
    runner
        .attach_lake(&LakeConnection {
            endpoint: &lake.endpoint,
            warehouse: &lake.warehouse,
            access_key: &lake.access_key,
            secret_key: &lake.secret_key,
            session_token: &lake.session_token,
            ca_file,
        })
        .map_err(engine("attaching the lake"))?;
    Ok(runner)
}

/// Reads the source's columns and creates the bronze table if it is not
/// there.
pub(crate) fn prepare(job: &JobDetails, ca_file: Option<&str>) -> Result<Plan, Failed> {
    let started = Instant::now();
    let cut = job
        .parameters
        .get("cut")
        .and_then(|c| c.as_str())
        .filter(|c| chrono_like_date(c))
        .ok_or_else(|| failed("extract: the job has no cut (parameters.cut, YYYY-MM-DD)"))?
        .to_string();
    let catalog = job
        .catalog
        .as_ref()
        .ok_or_else(|| failed("extract: the catalog has no mapping for the job's dataset"))?;
    let mapping = Mapping::parse(&catalog.mapping)
        .map_err(|_| failed("extract: the dataset's mapping does not parse"))?;
    let from = mapping
        .from
        .as_ref()
        .ok_or_else(|| failed("extract: the mapping names no source (from)"))?;
    let lake = job
        .lake
        .as_ref()
        .ok_or_else(|| failed("extract: no lake access came with the job"))?;
    let runner = connect(job, lake, ca_file)?;
    let (namespace, table) = layers::bronze(&mapping);
    let source_table = from.value.table.clone();
    let columns = runner
        .source_columns(&source_table)
        .map_err(engine("reading the source's columns"))?;
    let exists = runner
        .table_exists(&namespace, &table)
        .map_err(engine("looking for the bronze table"))?;
    if !exists {
        runner
            .create_bronze(&namespace, &table, &columns)
            .map_err(engine("creating the bronze table"))?;
    }
    let silver = layers::silver(&mapping);
    let mut created = !exists;
    if let Some((ns, t)) = &silver {
        if !runner
            .table_exists(ns, t)
            .map_err(engine("looking for the silver table"))?
        {
            runner
                .create_silver((ns, t), &mapping)
                .map_err(engine("creating the silver table"))?;
            created = true;
        }
    }
    Ok(Plan {
        cut,
        namespace,
        table,
        source_table,
        columns,
        mapping,
        silver,
        created,
        started,
    })
}

/// Replaces the cut with the source's rows, then silver's from them, with
/// `lake`'s credentials.
pub(crate) fn write(
    job: &JobDetails,
    plan: &Plan,
    lake: &LakeAccess,
    ca_file: Option<&str>,
) -> Result<ExtractReport, Failed> {
    let runner = connect(job, lake, ca_file)?;
    let (rows, snapshot) = runner
        .replace_cut(
            &plan.namespace,
            &plan.table,
            &plan.source_table,
            &plan.columns,
            &plan.cut,
        )
        .map_err(engine("replacing the cut"))?;
    let silver_snapshot = match &plan.silver {
        Some((ns, t)) => runner
            .replace_silver(&plan.mapping, (ns, t), &plan.cut, &snapshot)
            .map_err(engine("writing silver"))?
            .map(|(_, snapshot)| snapshot),
        None => None,
    };
    if plan.silver.is_some() && silver_snapshot.is_none() {
        tracing::warn!(
            "extract: a field's type changed and silver cannot take it: the cut is read from bronze"
        );
    }
    Ok(ExtractReport {
        cut: plan.cut.clone(),
        duration_ms: i64::try_from(plan.started.elapsed().as_millis()).unwrap_or(i64::MAX),
        source: plan.source_table.clone(),
        schema: plan
            .columns
            .iter()
            .map(|(name, ty)| SourceColumn {
                name: name.clone(),
                data_type: ty.clone(),
            })
            .collect(),
        ranges: vec![RangeRead {
            range: "all".into(),
            rows,
        }],
        snapshot,
        silver_snapshot,
    })
}

fn chrono_like_date(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn job(parameters: serde_json::Value) -> JobDetails {
        JobDetails {
            kind: "extract".into(),
            dataset: Some("loans".into()),
            parameters,
            engine: "duckdb".into(),
            attempts: 1,
            lake: None,
            catalog: None,
            source: None,
            entity_key: None,
            trino: None,
        }
    }

    #[test]
    fn an_extraction_without_what_it_needs_fails_saying_what() {
        assert_eq!(
            prepare(&job(json!({})), None).err().unwrap().0,
            "extract: the job has no cut (parameters.cut, YYYY-MM-DD)"
        );
        assert_eq!(
            prepare(&job(json!({"cut": "2021-03-10"})), None)
                .err()
                .unwrap()
                .0,
            "extract: the catalog has no mapping for the job's dataset"
        );
        assert!(prepare(&job(json!({"cut": "2021-3-10"})), None).is_err());
    }
}
