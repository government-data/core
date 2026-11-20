//! The findings of a cut and its queues (decision 0059) on four synthetic
//! loans in two offices, against the guide's priority (formula 5) worked by
//! hand; the record's hash checked against HMAC-SHA-256 computed apart.

use duckdb::Connection;
use hmac::{Hmac, Mac};
use quality::mapping::Mapping;
use quality::worker_api::QueueFindings;
use quality_rules_duckdb::{IcdRule, Previous, Runner};
use quality_rules_lint::lint;
use sha2::Sha256;

const MAPPING: &str = "dataset: loans
source: loans
key: [Key]
fields:
  Key: { column: id, type: text }
  Office: { column: office, type: text, roles: [dimension] }
  Phone: { column: phone, type: text, priority: critical, roles: [sfc] }
  Amount: { column: amount, type: number, priority: high }
";

const RULES: &str = "dataset: loans
rules:
  - { id: P1, field: Phone, check: required, severity: blocking, alert: a }

  - { id: A1, field: Amount, check: required, severity: major, alert: a }
";

/// A synthetic entity key, 64 characters as the tenant's.
const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const CUT: &str = "2026-03-31";

fn hmac(message: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(KEY.as_bytes()).unwrap();
    mac.update(message.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// L1 passes; L2 has no phone; L3 no amount; L4 neither.
fn loans(name: &str) -> (std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("qd-findings-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("loans.parquet").display().to_string();
    Connection::open_in_memory()
        .unwrap()
        .execute_batch(&format!(
            "COPY (SELECT * FROM (VALUES
                (' l1', 'O1', '300', 500.0),
                ('L2', 'O1', NULL, 2000.0),
                ('L3', 'O2', '310', NULL),
                ('L4', 'O2', NULL, NULL)
              ) AS t(id, office, phone, amount)) TO '{path}' (FORMAT parquet)"
        ))
        .unwrap();
    (dir, path)
}

fn run(mapping: &str, name: &str, previous: Option<Previous<'_>>) -> (Runner, Vec<QueueFindings>) {
    let (dir, path) = loans(name);
    let mapping = Mapping::parse(mapping).unwrap();
    let mut runner = Runner::new().unwrap();
    runner.load_mapped(&mapping, &path).unwrap();
    let report = lint(RULES, &runner.schema("loans", &[]).unwrap());
    assert_eq!(report.findings, []);
    let file = quality_rules_model::parse(RULES).unwrap();
    let tables = runner.tables(&file);
    let rules: Vec<IcdRule> = report
        .compiled
        .iter()
        .map(|r| IcdRule::of(r, &tables).unwrap())
        .collect();
    let icd = runner.icd("loans", &mapping, &rules, CUT).unwrap();
    let queues = runner
        .findings(&mapping, &rules, CUT, KEY, &icd.cells, previous)
        .unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    (runner, queues)
}

fn seen(queues: &[QueueFindings]) -> Vec<(&str, &str, i64, i64, f64)> {
    queues
        .iter()
        .map(|q| {
            assert_eq!(q.dimension, "Office");
            let p = (q.priority * 1e6).round() / 1e6;
            (
                q.group.as_deref().unwrap(),
                q.since.as_str(),
                q.open,
                q.resolved,
                p,
            )
        })
        .collect()
}

// Phone is in the indicator: right (filled, P1 passing) in 2 of 4, p = 0.5,
// closeness 0.3; its
// ICD on a failing record is 0, so P1's priority is 4/4 · 1 · 0.3. Amount
// is not: 3/4 · 1 · 0.2 = 0.15.
// Only L1 and L3 active: Phone is right in both, its cell's p = 1 and
// closeness 0.2, as the SFC view reads it; P1's priority 4/4 · 1 · 0.2.
// Inactive records still have their findings.
#[test]
fn a_findings_closeness_is_its_cells_among_the_active_records() {
    let active = MAPPING.replace(
        "Key: { column: id, type: text }",
        "Key: { column: id, type: text, roles: [active], active_values: [L1, L3] }",
    );
    let (_, queues) = run(&active, "active", None);
    assert_eq!(
        seen(&queues),
        [("O2", CUT, 3, 0, 0.5), ("O1", CUT, 1, 0, 0.2)]
    );
}

#[test]
fn each_failure_is_a_finding_with_the_guides_priority_by_office() {
    let (runner, queues) = run(MAPPING, "first", None);
    assert_eq!(
        seen(&queues),
        [("O2", CUT, 3, 0, 0.6), ("O1", CUT, 1, 0, 0.3)],
        "the office with the most open findings first"
    );
    let (_, rows) = runner
        .console("SELECT record, rule, round(priority, 6), since FROM \"__findings\" ORDER BY rule, record\n")
        .unwrap();
    // Opened at this cut, each record by the keyed hash of its key.
    let mut expected: Vec<Vec<Option<String>>> = [
        ("L3", "A1", "0.15"),
        ("L4", "A1", "0.15"),
        ("L2", "P1", "0.3"),
        ("L4", "P1", "0.3"),
    ]
    .iter()
    .map(|(k, rule, p)| {
        [hmac(k), (*rule).into(), (*p).into(), CUT.into()]
            .into_iter()
            .map(Some)
            .collect()
    })
    .collect();
    expected.sort_by(|a, b| (&a[1], &a[0]).cmp(&(&b[1], &b[0])));
    assert_eq!(rows, expected);
    let (_, groups) = runner
        .console("SELECT count(*) FROM \"__finding_groups\" WHERE dimension = 'Office'\n")
        .unwrap();
    assert_eq!(groups[0][0].as_deref(), Some("3"), "one per record");
    assert!(runner.console("SELECT * FROM \"__icd\"\n").is_err());
}

#[test]
fn since_is_carried_from_the_previous_cut_and_what_it_no_longer_has_resolves() {
    // L2's P1 opened in January; L1's A1 was open and passes now; L9 is gone.
    let findings = format!(
        "(SELECT * FROM (VALUES ('{}', 'P1', DATE '2026-01-31'), ('{}', 'A1', DATE '2026-02-28'),
                               ('{}', 'P1', DATE '2026-02-28')) AS t(record, rule, since))",
        hmac("L2"),
        hmac("L1"),
        hmac("L9")
    );
    let groups = format!(
        "(SELECT * FROM (VALUES ('{}', 'Office', 'O1'), ('{}', 'Office', 'O1'),
                               ('{}', 'Office', 'O3')) AS t(record, dimension, grp))",
        hmac("L2"),
        hmac("L1"),
        hmac("L9")
    );
    let previous = Previous {
        findings: &findings,
        groups: &groups,
    };
    let (_, queues) = run(MAPPING, "second", Some(previous));
    assert_eq!(
        seen(&queues),
        [
            ("O2", CUT, 3, 0, 0.6),
            ("O1", "2026-01-31", 1, 0, 0.3),
            ("O1", "2026-02-28", 0, 1, 0.0),
            ("O3", "2026-02-28", 0, 1, 0.0),
        ]
    );
}

#[test]
fn a_dataset_without_a_key_nor_an_entity_has_no_findings() {
    let (_, queues) = run(&MAPPING.replace("key: [Key]\n", ""), "keyless", None);
    assert!(queues.is_empty());
}
