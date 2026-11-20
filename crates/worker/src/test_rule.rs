//! The `test_rule` job (decisions 0019 point 10, 0043 point 3): a rules
//! file as typed, run on the dataset's cut it came with, read through its
//! mapping from the lake with the job's own credentials. It brings each
//! rule's counts and a read of a few records of each outcome, with the
//! key, the fields the rule reads and those asked for (`columns`): the API
//! masks and audits them; they
//! never reach a log here.

use quality::catalog::{CatalogSnapshot, RulesSource};
use quality::rule_test::{FailGroup, FieldValue, RuleTested, TestReport, TestedRecord};
use quality::worker_api::JobDetails;
use quality_rules::lint::{lint_with, Severity};
use quality_rules::model::GLOBAL_FILE;
use quality_rules_duckdb::{LakeConnection, Outcome, Runner, Shown, SilverAt};

use crate::evaluate::{engine, failed, global_rules, is_date, merged, parse_mapping, Failed};

/// What the job came with, checked.
struct Asked<'a> {
    dataset: &'a str,
    cut: &'a str,
    snapshot: Option<i64>,
    yaml: &'a str,
    rule: Option<&'a str>,
    each: usize,
    /// Fields shown besides the key and those a rule reads.
    columns: Vec<String>,
}

fn asked(job: &JobDetails) -> Result<Asked<'_>, Failed> {
    let p = &job.parameters;
    let text = |name: &str| p.get(name).and_then(|v| v.as_str());
    Ok(Asked {
        dataset: job
            .dataset
            .as_deref()
            .ok_or_else(|| failed("test: the job has no dataset"))?,
        cut: text("cut")
            .filter(|c| is_date(c))
            .ok_or_else(|| failed("test: the job has no cut (parameters.cut, YYYY-MM-DD)"))?,
        snapshot: text("snapshot").and_then(|s| s.parse().ok()),
        yaml: text("yaml").ok_or_else(|| failed("test: the job has no rules file"))?,
        rule: text("rule"),
        each: p
            .get("each")
            .and_then(serde_json::Value::as_u64)
            .map_or(10, |n| usize::try_from(n.min(100)).unwrap_or(10)),
        columns: p
            .get("columns")
            .and_then(serde_json::Value::as_array)
            .map(|c| {
                c.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Runs the test; `ca_file` is a CA to trust for the lake besides the
/// system's.
pub(crate) fn test_rule(job: &JobDetails, ca_file: Option<&str>) -> Result<TestReport, Failed> {
    let a = asked(job)?;
    let lake = job
        .lake
        .as_ref()
        .ok_or_else(|| failed("test: no lake access came with the job"))?;
    let catalog = job.catalog.as_ref().ok_or_else(|| {
        failed(format!(
            "test: the catalog has no mapping for {}",
            a.dataset
        ))
    })?;
    let mapping = parse_mapping(a.dataset, &catalog.mapping)?;
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
        .map_err(engine("attaching the lake"))?;
    runner
        .load_mapped_lake(&mapping, a.cut, a.snapshot, Some(SilverAt::Current))
        .map_err(engine(&format!("reading {}", a.dataset)))?;
    // The references the text and the global rules read, not those of
    // the other rules on `main`.
    let globals = global_rules(catalog).map(str::to_string);
    let mut rules = vec![RulesSource {
        file: "test.yaml".into(),
        yaml: a.yaml.to_string(),
    }];
    rules.extend(globals.as_ref().map(|yaml| RulesSource {
        file: GLOBAL_FILE.into(),
        yaml: yaml.clone(),
    }));
    let typed = CatalogSnapshot {
        rules,
        ..catalog.clone()
    };
    crate::references::load_references(&mut runner, &typed, a.cut)?;
    run(&runner, &a, &mapping.key, globals.as_deref())
}

/// The largest groups of a rule's failures brought.
const GROUPS: usize = 20;

/// A record's values by field, as the report says them.
fn values(v: Vec<(String, Option<String>)>) -> Vec<FieldValue> {
    v.into_iter()
        .map(|(field, value)| FieldValue { field, value })
        .collect()
}

/// Lints the text against the cut's schema, then counts and reads each
/// rule asked (one, or every rule of the file), with the table's global
/// rules (decision 0045).
fn run(
    runner: &Runner,
    a: &Asked<'_>,
    key: &[String],
    globals: Option<&str>,
) -> Result<TestReport, Failed> {
    let schema = runner
        .schema(a.dataset, &[])
        .map_err(engine("reading the schema"))?;
    let report = lint_with(a.yaml, &schema, globals);
    if report
        .findings
        .iter()
        .any(|f| f.severity() == Severity::Error)
    {
        return Err(failed("test: the rules file has errors against the cut"));
    }
    let file =
        merged(a.yaml, globals).ok_or_else(|| failed("test: the rules file does not parse"))?;
    let tables = runner.tables(&file);
    let compiled: Vec<_> = report
        .compiled
        .into_iter()
        .filter(|r| {
            a.rule
                .is_none_or(|id| r.id == id || r.id.starts_with(&format!("{id}.")))
        })
        .collect();
    let counts = runner
        .test(a.dataset, &compiled, &tables, a.cut)
        .map_err(engine("counting"))?;
    let shown = |key| Shown {
        key,
        extra: &a.columns,
    };
    let mut rules = Vec::new();
    for (rule, c) in compiled.iter().zip(counts) {
        let sampled = runner
            .sample(a.dataset, rule, &tables, shown(key), a.cut, a.each)
            .map_err(engine(&format!("reading records of {}", rule.id)))?;
        // Every failure, by the shape of what the rule read (0043).
        let groups = if c.fails > 0 {
            runner
                .fail_groups(
                    a.dataset,
                    rule,
                    &tables,
                    shown(key),
                    a.cut,
                    (GROUPS, a.each),
                )
                .map_err(engine(&format!("grouping the failures of {}", rule.id)))?
        } else {
            Vec::new()
        };
        rules.push(RuleTested {
            rule: c.rule,
            applies: c.applies,
            passes: c.passes,
            fails: c.fails,
            skips: c.skipped,
            records: sampled
                .into_iter()
                .map(|s| TestedRecord {
                    outcome: match s.outcome {
                        Outcome::Fails => "fails",
                        Outcome::Passes => "passes",
                        Outcome::Skips => "skips",
                    }
                    .into(),
                    values: values(s.values),
                })
                .collect(),
            groups: groups
                .into_iter()
                .map(|g| FailGroup {
                    shape: g.shape,
                    count: g.count,
                    records: g
                        .records
                        .into_iter()
                        .map(|v| TestedRecord {
                            outcome: "fails".into(),
                            values: values(v),
                        })
                        .collect(),
                })
                .collect(),
        });
    }
    Ok(TestReport {
        cut: a.cut.to_string(),
        records: runner
            .records(a.dataset)
            .map_err(engine("counting records"))?,
        rules,
    })
}
