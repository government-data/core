//! The job users' neutral grants (decision 0090, point 7.2, ruling 17):
//! what each job user may do on the lake and the sources, in the shape
//! `engine-trino`'s rules merge takes (`{subject, namespace, tables,
//! privileges, rows}`), never in Trino's syntax. Pure: the caller gives
//! the datasets, their sources and the datasets their rules reference.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::layers::namespaces;

/// Gold's namespace in the warehouse.
pub const GOLD: &str = "quality_gold";
/// Gold's table of jobs' counts (ruling 53).
pub const JOB_COUNTS: &str = "job_counts";
/// The scratch namespace and its one table (point 8).
pub const SCRATCH: &str = "quality_scratch";
pub const SCRATCH_TABLE: &str = "record_icd";

/// A privilege a grant gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Privilege {
    Select,
    Insert,
    Delete,
}

use Privilege::{Delete, Insert, Select};

/// The rows of a table a grant gives: those whose `column` is in `in`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rows {
    pub column: String,
    #[serde(rename = "in")]
    pub values: Vec<String>,
}

/// One neutral grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Grant {
    pub subject: String,
    /// A schema of the warehouse, or `<catalog>.<schema>` for a source.
    pub namespace: String,
    pub tables: Vec<String>,
    pub privileges: Vec<Privilege>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<Rows>,
}

/// A dataset as its jobs see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetJobs {
    pub dataset: String,
    /// The sources its extraction reads.
    pub sources: Vec<String>,
    /// The other datasets its approved rules reference.
    pub references: Vec<String>,
    /// Its bronze and silver table (the mapping's `source`), whose `$refs`
    /// its report user reads to pin the job (ruling 39).
    pub table: Option<String>,
}

/// A name the rules merge takes: `[a-z0-9_]`, 1 to 63.
pub fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn grant(subject: &str, namespace: &str, tables: &[&str], privileges: &[Privilege]) -> Grant {
    Grant {
        subject: subject.into(),
        namespace: namespace.into(),
        tables: tables.iter().map(|t| (*t).to_string()).collect(),
        privileges: privileges.to_vec(),
        rows: None,
    }
}

fn of_dataset(mut g: Grant, dataset: &str) -> Grant {
    g.rows = Some(Rows {
        column: "dataset".into(),
        values: vec![dataset.into()],
    });
    g
}

/// The grants of every job user: per dataset its `rules`, `report`,
/// `extract`, `sample` and `maint` users, `job_maint_gold`, and one
/// `job_source_<source>` per source. A dataset, source or reference whose
/// name the merge would refuse gets no grant.
pub fn job_grants(datasets: &[DatasetJobs]) -> Vec<Grant> {
    let mut out = Vec::new();
    let mut sources = BTreeSet::new();
    let tables: BTreeMap<&str, &str> = datasets
        .iter()
        .filter(|d| is_name(&d.dataset))
        .filter_map(|d| {
            Some((
                d.dataset.as_str(),
                d.table.as_deref().filter(|t| is_name(t))?,
            ))
        })
        .collect();
    for d in datasets.iter().filter(|d| is_name(&d.dataset)) {
        let srcs: Vec<&str> = d
            .sources
            .iter()
            .map(String::as_str)
            .filter(|s| is_name(s))
            .collect();
        sources.extend(srcs.iter().copied());
        dataset_grants(&mut out, d, &srcs, &tables);
    }
    out.push(grant("job_maint_gold", GOLD, &["*"], &[Select, Delete]));
    for s in sources {
        out.push(grant(
            &format!("job_source_{s}"),
            &format!("src_{s}.*"),
            &["*"],
            &[Select],
        ));
    }
    out
}

/// `user`'s `SELECT` on `dataset`'s bronze and silver `<table>$refs` only.
fn refs_grants(out: &mut Vec<Grant>, user: &str, dataset: &str, table: &str) {
    let refs = format!("{table}$refs");
    for ns in &namespaces(dataset) {
        out.push(grant(user, ns, &[refs.as_str()], &[Select]));
    }
}

fn dataset_grants(
    out: &mut Vec<Grant>,
    d: &DatasetJobs,
    sources: &[&str],
    tables: &BTreeMap<&str, &str>,
) {
    let name = d.dataset.as_str();
    let layers = namespaces(name);
    let rules = format!("job_rules_{name}");
    let mut read: BTreeSet<String> = layers.iter().cloned().collect();
    for r in d.references.iter().filter(|r| is_name(r) && *r != name) {
        read.extend(namespaces(r));
    }
    for ns in &read {
        out.push(grant(&rules, ns, &["*"], &[Select]));
    }
    // Trino refuses a filtered INSERT or DELETE without SELECT (T4).
    let writes = [Select, Insert, Delete];
    out.push(of_dataset(grant(&rules, GOLD, &["*"], &writes), name));
    out.push(of_dataset(
        grant(&rules, SCRATCH, &[SCRATCH_TABLE], &writes),
        name,
    ));
    let report = format!("job_report_{name}");
    out.push(of_dataset(grant(&report, GOLD, &["*"], &[Select]), name));
    // Snapshot ids only: one exact `<table>$refs` per layer, no wildcard
    // (ruling 39; the spelling settled with `trino-rules` 2026-11-25).
    if let Some(t) = d.table.as_deref().filter(|t| is_name(t)) {
        refs_grants(out, &report, name, t);
    }
    // And its references' (ruling 61): the evaluate pins them too.
    for r in d.references.iter().filter(|r| is_name(r) && *r != name) {
        if let Some(t) = tables.get(r.as_str()) {
            refs_grants(out, &report, r, t);
        }
    }
    let extract = format!("job_extract_{name}");
    for s in sources {
        out.push(grant(&extract, &format!("src_{s}.*"), &["*"], &[Select]));
    }
    let sample = format!("job_sample_{name}");
    let maint = format!("job_maint_{name}");
    // A retry deletes its own run's earlier load from bronze, after a
    // count of it through gold's `job_counts` (ruling 67); the bundle's
    // statement names the load.
    let [bronze, silver] = &layers;
    out.push(grant(&extract, bronze, &["*"], &[Select, Insert, Delete]));
    out.push(grant(&extract, silver, &["*"], &[Select, Insert]));
    out.push(of_dataset(
        grant(&extract, GOLD, &[JOB_COUNTS], &writes),
        name,
    ));
    for ns in &layers {
        out.push(grant(&sample, ns, &["*"], &[Select]));
        out.push(grant(&maint, ns, &["*"], &writes));
    }
    out.push(of_dataset(
        grant(&maint, SCRATCH, &[SCRATCH_TABLE], &writes),
        name,
    ));
}

/// A console statement's run as its grants see it (decision 0090, ruling
/// 84): its user's target (`r` and the run's hex) and the datasets its
/// asker may view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleRun {
    pub target: String,
    pub datasets: Vec<String>,
}

/// Each open console run's user, `job_console_<target>`: `SELECT` on the
/// bronze and silver of its datasets, and on `gold_tables` for those
/// datasets' rows only; never a source, never another dataset.
pub fn console_grants(runs: &[ConsoleRun], gold_tables: &[&str]) -> Vec<Grant> {
    let mut out = Vec::new();
    for run in runs.iter().filter(|r| is_name(&r.target)) {
        let user = format!("job_console_{}", run.target);
        let datasets: Vec<String> = run
            .datasets
            .iter()
            .filter(|d| is_name(d))
            .cloned()
            .collect();
        if datasets.is_empty() {
            continue;
        }
        for d in &datasets {
            for ns in &namespaces(d) {
                out.push(grant(&user, ns, &["*"], &[Select]));
            }
        }
        let mut gold = grant(&user, GOLD, gold_tables, &[Select]);
        gold.rows = Some(Rows {
            column: "dataset".into(),
            values: datasets,
        });
        out.push(gold);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_console_run_reads_only_its_askers_datasets() {
        let runs = [
            ConsoleRun {
                target: "r0b9c6a1e000040008000000000000091".into(),
                datasets: vec!["loans".into(), "Bad-Name".into()],
            },
            ConsoleRun {
                target: "rnone".into(),
                datasets: vec![],
            },
        ];
        let g = console_grants(&runs, &["run_summary"]);
        let user = "job_console_r0b9c6a1e000040008000000000000091";
        assert!(g.iter().all(|g| g.subject == user));
        let namespaces: Vec<&str> = g.iter().map(|g| g.namespace.as_str()).collect();
        assert_eq!(namespaces, ["loans_bronze", "loans_silver", GOLD]);
        assert_eq!(
            g[2].rows,
            Some(Rows {
                column: "dataset".into(),
                values: vec!["loans".into()]
            })
        );
        assert_eq!(g[2].tables, ["run_summary"]);
    }
}
