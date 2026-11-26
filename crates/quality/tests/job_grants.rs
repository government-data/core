//! The job users' neutral grants (decision 0090, point 7.2).

use quality::job_grants::{job_grants, DatasetJobs, Grant};
use serde_json::{json, Value};

fn loans() -> DatasetJobs {
    DatasetJobs {
        dataset: "loans".into(),
        sources: vec!["crm".into()],
        references: vec!["branches".into(), "loans".into()],
        table: None,
    }
}

fn of<'a>(grants: &'a [Grant], subject: &str) -> Vec<&'a Grant> {
    grants.iter().filter(|g| g.subject == subject).collect()
}

#[test]
fn a_rules_job_reads_its_layers_and_references_and_writes_its_gold_rows() {
    let g = job_grants(&[loans()]);
    let rules: Vec<Value> = of(&g, "job_rules_loans")
        .into_iter()
        .map(|g| serde_json::to_value(g).unwrap())
        .collect();
    let rows = json!({"column": "dataset", "in": ["loans"]});
    assert_eq!(
        rules,
        [
            json!({"subject": "job_rules_loans", "namespace": "branches_bronze", "tables": ["*"], "privileges": ["SELECT"]}),
            json!({"subject": "job_rules_loans", "namespace": "branches_silver", "tables": ["*"], "privileges": ["SELECT"]}),
            json!({"subject": "job_rules_loans", "namespace": "loans_bronze", "tables": ["*"], "privileges": ["SELECT"]}),
            json!({"subject": "job_rules_loans", "namespace": "loans_silver", "tables": ["*"], "privileges": ["SELECT"]}),
            json!({"subject": "job_rules_loans", "namespace": "quality_gold", "tables": ["*"],
                   "privileges": ["SELECT", "INSERT", "DELETE"], "rows": rows}),
            json!({"subject": "job_rules_loans", "namespace": "quality_scratch", "tables": ["record_icd"],
                   "privileges": ["SELECT", "INSERT", "DELETE"], "rows": rows}),
        ]
    );
}

#[test]
fn each_kind_gets_only_its_own_places() {
    let g = job_grants(&[loans()]);
    let report = of(&g, "job_report_loans");
    assert_eq!(report.len(), 1);
    assert_eq!(report[0].namespace, "quality_gold");
    assert!(report[0].rows.is_some());
    let places = |s| -> Vec<String> { of(&g, s).iter().map(|g| g.namespace.clone()).collect() };
    assert_eq!(
        places("job_extract_loans"),
        ["src_crm.*", "loans_bronze", "loans_silver"]
    );
    assert_eq!(places("job_sample_loans"), ["loans_bronze", "loans_silver"]);
    assert_eq!(
        places("job_maint_loans"),
        ["loans_bronze", "loans_silver", "quality_scratch"]
    );
    assert_eq!(places("job_source_crm"), ["src_crm.*"]);
    let gold = of(&g, "job_maint_gold");
    assert_eq!((gold.len(), gold[0].rows.is_none()), (1, true));
    // People never get a grant here, and nothing touches another dataset's
    // gold rows.
    assert!(g.iter().all(|g| g.subject.starts_with("job_")));
    assert!(g
        .iter()
        .filter_map(|g| g.rows.as_ref())
        .all(|r| r.values == ["loans"]));
}

#[test]
fn a_name_the_merge_would_refuse_gets_no_grant() {
    let bad = DatasetJobs {
        dataset: "Loans-2".into(),
        sources: vec!["crm".into()],
        references: vec![],
        table: None,
    };
    let odd = DatasetJobs {
        dataset: "savings".into(),
        sources: vec!["c.rm".into()],
        references: vec!["x'y".into()],
        table: None,
    };
    let g = job_grants(&[bad, odd]);
    assert!(g.iter().all(|g| !g.subject.contains("Loans")));
    assert!(g
        .iter()
        .all(|g| !g.namespace.contains("c.rm") && !g.namespace.contains('\'')));
    assert!(of(&g, "job_source_crm").is_empty());
    assert_eq!(of(&g, "job_rules_savings").len(), 4);
}

#[test]
fn the_report_user_reads_its_tables_refs_by_exact_name() {
    let mut d = loans();
    d.table = Some("loans_extract".into());
    let g = job_grants(&[d.clone()]);
    let refs: Vec<Value> = of(&g, "job_report_loans")
        .into_iter()
        .filter(|g| g.namespace != "quality_gold")
        .map(|g| serde_json::to_value(g).unwrap())
        .collect();
    assert_eq!(
        refs,
        [
            json!({"subject": "job_report_loans", "namespace": "loans_bronze",
                   "tables": ["loans_extract$refs"], "privileges": ["SELECT"]}),
            json!({"subject": "job_report_loans", "namespace": "loans_silver",
                   "tables": ["loans_extract$refs"], "privileges": ["SELECT"]}),
        ]
    );
    d.table = Some("x$snapshots".into());
    let g = job_grants(&[d]);
    assert!(g.iter().all(|g| g.tables.iter().all(|t| !t.contains('$'))));
}

/// Ruling 61: the report user also reads its references' `$refs`, by the
/// referenced dataset's own table, to pin them; a reference with no known
/// table, or the dataset itself, adds nothing.
#[test]
fn the_report_user_reads_its_references_refs() {
    let mut d = loans();
    d.table = Some("loans_extract".into());
    let branches = DatasetJobs {
        dataset: "branches".into(),
        sources: vec![],
        references: vec![],
        table: Some("offices".into()),
    };
    let g = job_grants(&[d.clone(), branches]);
    let refs: Vec<(String, Vec<String>)> = of(&g, "job_report_loans")
        .into_iter()
        .filter(|g| g.namespace != "quality_gold")
        .map(|g| (g.namespace.clone(), g.tables.clone()))
        .collect();
    assert_eq!(
        refs,
        [
            ("loans_bronze".into(), vec!["loans_extract$refs".into()]),
            ("loans_silver".into(), vec!["loans_extract$refs".into()]),
            ("branches_bronze".into(), vec!["offices$refs".into()]),
            ("branches_silver".into(), vec!["offices$refs".into()]),
        ]
    );
    let alone = job_grants(&[d]);
    assert_eq!(
        of(&alone, "job_report_loans")
            .iter()
            .filter(|g| g.namespace.starts_with("branches"))
            .count(),
        0
    );
}
