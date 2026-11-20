//! The `evaluate` job (decisions 0019, 0031, 0061): a dataset's rules, from
//! the catalog snapshot the job came with, over one cut of its silver table
//! (or its bronze one, typed as mapped) in the tenant's lake, attached with
//! the job's own credentials. Counts per rule, each field's profile, the
//! ICD (decision 0057) and the remediation queues (decision 0059) come out,
//! never a value: an entity and a record only by their hashes
//! ([`crate::icd`], [`crate::findings`]); and how long each stage took
//! ([`crate::stages`]).
//!
//! Its errors go to the queue (`jobs.error`) and the log, so they say what
//! failed in our terms (a file, a rule code, a dataset), never an engine's
//! message, which may quote a value.

use std::collections::BTreeSet;
use std::time::Instant;

use quality::catalog::CatalogSnapshot;
use quality::lake::LakeAccess;
use quality::mapping::Mapping;
use quality::worker_api::{CompiledRule, JobDetails, Report, RuleCounts, RulesFileRun};
use quality_rules::lint::{lint_globals, lint_with, Severity};
use quality_rules::model::{with_globals, RulesFile, GLOBAL_FILE};
use quality_rules::sql::Engine;
use quality_rules_duckdb::{IcdRule, LakeConnection, Runner, SilverAt};
use sha2::{Digest, Sha256};

use crate::stages::Stages;

/// Why a job could not be evaluated, safe to store and to log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failed(pub String);

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Failed {}

pub(crate) fn failed(message: impl Into<String>) -> Failed {
    Failed(message.into())
}

/// What the engine said, reduced to where it happened.
pub(crate) fn engine(stage: &str) -> impl Fn(quality_rules_duckdb::Error) -> Failed + '_ {
    move |_| failed(format!("evaluate: the engine failed while {stage}"))
}

/// Runs the job; `ca_file` is a CA to trust for the lake besides the
/// system's.
pub(crate) fn evaluate(job: &JobDetails, ca_file: Option<&str>) -> Result<Report, Failed> {
    let started = Instant::now();
    let cut = job
        .parameters
        .get("cut")
        .and_then(|c| c.as_str())
        .filter(|c| is_date(c))
        .ok_or_else(|| failed("evaluate: the job has no cut (parameters.cut, YYYY-MM-DD)"))?;
    let dataset = job
        .dataset
        .as_deref()
        .ok_or_else(|| failed("evaluate: the job has no dataset"))?;
    let lake = job
        .lake
        .as_ref()
        .ok_or_else(|| failed("evaluate: no lake access came with the job"))?;
    let catalog = job.catalog.as_ref().ok_or_else(|| {
        failed(format!(
            "evaluate: the catalog has no mapping for {dataset}"
        ))
    })?;
    let mapping = parse_mapping(dataset, &catalog.mapping)?;
    let pinned = pinned(job)?;
    let mut stages = Stages::start();
    let mut runner = attach(lake, ca_file)?;
    let read = runner
        .load_mapped_lake(&mapping, cut, pinned, silver_at(job, pinned)?)
        .map_err(engine(&format!("reading {dataset}")))?;
    let mut snapshots = vec![crate::references::snapshot_read(dataset, read)];
    snapshots.extend(crate::references::load_references(
        &mut runner,
        catalog,
        cut,
    )?);
    let records = runner
        .records(dataset)
        .map_err(engine("counting records"))?;
    stages.lap("load");

    let Ran {
        rules,
        files,
        compiled,
        icd: measured,
    } = run_files(&runner, dataset, catalog, cut)?;
    stages.lap("rules");
    let fields: Vec<&str> = mapping.fields.keys().map(String::as_str).collect();
    let mut fields = runner
        .field_quality(dataset, &fields)
        .map_err(engine("profiling the fields"))?;
    stages.lap("profile");
    let icd = runner
        .icd(dataset, &mapping, &measured, cut)
        .map_err(engine("computing the ICD"))?;
    crate::icd::merge(&mut fields, &icd.fields);
    stages.lap("icd");
    let queues = crate::findings::queues(
        &runner,
        &mapping,
        &measured,
        cut,
        &icd.cells,
        job.entity_key.as_ref(),
    )?;
    stages.lap("queues");
    crate::distribution::recency(&runner, dataset, &mapping, cut, &mut fields)?;
    stages.lap("recency");
    let distributions = crate::distribution::distributions(&runner, dataset, &mapping)?;
    let outliers = crate::distribution::outliers(&runner, dataset, &mapping)?;
    stages.lap("distributions");
    Ok(Report {
        cut: cut.to_string(),
        records: count(records),
        duration_ms: i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
        rules_commit: catalog.commit.clone(),
        rules,
        snapshots,
        files,
        compiled,
        compiler: quality_rules::VERSION.to_string(),
        fields,
        igcd: icd.igcd,
        entities: crate::icd::hashed(&icd.entities, job.entity_key.as_ref()),
        groups: icd.groups,
        cells: icd.cells,
        queues,
        distributions,
        outliers,
        stages: stages.done(),
    })
}

/// The snapshot the job pins, if any: a cut's evaluation reads the one its
/// extraction committed.
fn pinned(job: &JobDetails) -> Result<Option<i64>, Failed> {
    job.parameters
        .get("snapshot")
        .map(|s| {
            s.as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .ok_or_else(|| failed("evaluate: parameters.snapshot is not a snapshot id"))
        })
        .transpose()
}

/// Which silver the evaluation reads (decision 0061): the snapshot its
/// extraction wrote; with none, silver's current one unless bronze is
/// pinned (a cut extracted before silver, or through Trino: bronze).
fn silver_at(job: &JobDetails, pinned: Option<i64>) -> Result<Option<SilverAt>, Failed> {
    match job.parameters.get("silver_snapshot") {
        Some(s) => s
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .map(|id| Some(SilverAt::Pinned(id)))
            .ok_or_else(|| failed("evaluate: parameters.silver_snapshot is not a snapshot id")),
        None => Ok(pinned.is_none().then_some(SilverAt::Current)),
    }
}

/// An engine with the tenant's lake attached.
fn attach(lake: &LakeAccess, ca_file: Option<&str>) -> Result<Runner, Failed> {
    let mut runner = Runner::new().map_err(engine("starting"))?;
    runner
        .attach_lake(&LakeConnection {
            endpoint: &lake.endpoint,
            warehouse: &lake.warehouse,
            access_key: &lake.access_key,
            secret_key: &lake.secret_key,
            session_token: &lake.session_token,
            ca_file,
        })
        .map_err(|e| {
            // No record has been read yet: the engine's message names a
            // setting, a file or an address, never a value.
            tracing::warn!("attaching the lake: {e}");
            engine("attaching the lake")(e)
        })?;
    Ok(runner)
}

/// What the rules files gave: counts, and what ran for the manifest.
#[derive(Default)]
struct Ran {
    rules: Vec<RuleCounts>,
    files: Vec<RulesFileRun>,
    compiled: Vec<CompiledRule>,
    /// Every rule of every file, as the ICD reads them.
    icd: Vec<IcdRule>,
}

/// Lints each rules file of the snapshot against the loaded dataset, then
/// runs it; a file with lint errors fails the job naming its codes.
fn run_files(
    runner: &Runner,
    dataset: &str,
    catalog: &CatalogSnapshot,
    cut: &str,
) -> Result<Ran, Failed> {
    let schema = runner
        .schema(dataset, &[])
        .map_err(engine("reading the schema"))?;
    let mut ran = Ran::default();
    let globals = global_rules(catalog);
    for source in &catalog.rules {
        // The global rules run in each file that uses them, not alone.
        let global = source.file == GLOBAL_FILE;
        let report = if global {
            lint_globals(&source.yaml, &schema)
        } else {
            lint_with(&source.yaml, &schema, globals)
        };
        let mut findings = report.findings.clone();
        findings.extend(
            runner
                .examples(&report.examples, cut)
                .map_err(engine("running the examples"))?,
        );
        let errors: BTreeSet<&str> = findings
            .iter()
            .filter(|f| f.severity() == Severity::Error)
            .map(|f| f.code.id())
            .collect();
        if !errors.is_empty() {
            let codes: Vec<&str> = errors.into_iter().collect();
            return Err(failed(format!(
                "evaluate: {} has errors ({})",
                source.file,
                codes.join(", ")
            )));
        }
        if global {
            continue;
        }
        let file = merged(&source.yaml, globals)
            .ok_or_else(|| failed(format!("evaluate: {} does not parse", source.file)))?;
        let tables = runner.tables(&file);
        let counts = runner
            .test(dataset, &report.compiled, &tables, cut)
            .map_err(engine(&format!("running {}", source.file)))?;
        // What ran, for the manifest: the file's hash and each rule's SQL.
        ran.files.push(RulesFileRun {
            file: source.file.clone(),
            sha256: hex(&Sha256::digest(source.yaml.as_bytes())),
        });
        for rule in &report.compiled {
            ran.icd
                .push(IcdRule::of(rule, &tables).map_err(engine("compiling for the ICD"))?);
            if let Ok(sql) = quality_rules::sql::rule(rule, Engine::DuckDb, &tables) {
                ran.compiled.push(CompiledRule {
                    rule: rule.id.clone(),
                    sql: sql.text,
                });
            }
        }
        // The counts come in the compiled rules' order.
        ran.rules.extend(
            counts
                .into_iter()
                .zip(&report.compiled)
                .map(|(c, rule)| RuleCounts {
                    rule: c.rule,
                    applies: count(c.applies),
                    failures: count(c.fails),
                    field: rule.field.clone(),
                    pattern: Some(rule.pattern.name().to_string()),
                    dimension: Some(rule.dimension.name().to_string()),
                    severity: Some(rule.severity.name().to_string()),
                }),
        );
    }
    Ok(ran)
}

/// The table's global rules among the catalog's files (decision 0045).
pub(crate) fn global_rules(catalog: &CatalogSnapshot) -> Option<&str> {
    catalog
        .rules
        .iter()
        .find(|r| r.file == GLOBAL_FILE)
        .map(|r| r.yaml.as_str())
}

/// A rules file as it runs: with the table's global rules, if any.
pub(crate) fn merged(yaml: &str, globals: Option<&str>) -> Option<RulesFile> {
    let file = quality_rules::parse(yaml).ok()?;
    Some(match globals.and_then(|g| quality_rules::parse(g).ok()) {
        Some(globals) => with_globals(&file, &globals).0,
        None => file,
    })
}

pub(crate) fn parse_mapping(dataset: &str, yaml: &str) -> Result<Mapping, Failed> {
    Mapping::parse(yaml)
        .map_err(|_| failed(format!("evaluate: the mapping of {dataset} does not parse")))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn count(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

pub(crate) fn is_date(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_file_runs_with_the_tables_global_rules() {
        let globals = "dataset: d\nnamed:\n  filled:\n    check: required\n";
        let yaml = "dataset: d\nrules:\n  - id: A-01\n    field: A\n    extends: filled\n";
        let file = merged(yaml, Some(globals)).unwrap();
        assert_eq!(
            file.rules[0].check().value,
            quality_rules::model::Pattern::Required
        );
        assert!(merged(yaml, None).unwrap().rules[0].check.is_none());
        assert!(merged("not: [a rules file", None).is_none());
    }

    fn job(parameters: serde_json::Value) -> JobDetails {
        JobDetails {
            kind: "evaluate".into(),
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
    fn a_job_without_what_it_needs_fails_saying_what() {
        assert_eq!(
            evaluate(&job(json!({})), None).unwrap_err().0,
            "evaluate: the job has no cut (parameters.cut, YYYY-MM-DD)"
        );
        assert!(evaluate(&job(json!({"cut": "2021-3-10"})), None).is_err());
        assert_eq!(
            evaluate(&job(json!({"cut": "2021-03-10"})), None)
                .unwrap_err()
                .0,
            "evaluate: no lake access came with the job"
        );
    }

    #[test]
    fn an_evaluation_reads_the_silver_its_extraction_wrote_or_bronze_as_pinned() {
        let silver = job(json!({"snapshot": "7", "silver_snapshot": "8"}));
        assert_eq!(silver_at(&silver, Some(7)), Ok(Some(SilverAt::Pinned(8))));
        assert_eq!(silver_at(&job(json!({})), Some(7)), Ok(None));
        assert_eq!(
            silver_at(&job(json!({})), None),
            Ok(Some(SilverAt::Current))
        );
        assert!(silver_at(&job(json!({"silver_snapshot": "x"})), None).is_err());
    }
}
