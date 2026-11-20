//! The ICD within a dataset (decision 0057) on three synthetic loans, two of
//! one client whose document is written two ways, against the guide's
//! formulas worked by hand.

use duckdb::Connection;
use quality::mapping::Mapping;
use quality_rules_duckdb::{band, IcdRule, Runner};
use quality_rules_lint::lint;

const MAPPING: &str = "dataset: loans
source: loans
fields:
  Doc: { column: doc, type: text, semantic: document, roles: [entity] }
  Type: { column: kind, type: text, roles: [segment] }
  Phone: { column: phone, type: text, priority: critical, expires: true }
  Amount: { column: amount, type: number, priority: high }
  Updated: { column: updated, type: date, roles: [updated] }
";

// Phone's own rule (C, blocking), Amount's (C, major), and one across
// Amount and Type (S of both, minor).
const RULES: &str = "dataset: loans
rules:
  - { id: P1, field: Phone, check: required, severity: blocking, alert: a }

  - { id: A1, field: Amount, check: required, severity: major, alert: a }

  - id: X1
    check: expression
    fails: Amount > 1000 AND Type = 'PN'
    dimension: consistency
    severity: minor
    alert: a
";

fn loans(name: &str) -> (std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("qd-icd-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("loans.parquet").display().to_string();
    Connection::open_in_memory()
        .unwrap()
        .execute_batch(&format!(
            "COPY (SELECT * FROM (VALUES
                ('001.234', 'PN', '300', 500.0, DATE '2026-12-31'),
                ('1234', 'PN', NULL, 2000.0, DATE '2025-12-31'),
                ('999', 'PN', '310', NULL, NULL)
              ) AS t(doc, kind, phone, amount, updated)) TO '{path}' (FORMAT parquet)"
        ))
        .unwrap();
    (dir, path)
}

fn icd(mapping: &str, name: &str) -> quality_rules_duckdb::IcdReport {
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
    let out = runner.icd("loans", &mapping, &rules, "2026-12-31").unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    out
}

fn near(a: Option<f64>, b: f64) -> bool {
    a.is_some_and(|a| (a - b).abs() < 1e-6)
}

// Per record (C 0.40, S 0.15, A 0.15; Phone 4, Amount 3, Type 2):
// 1: everything passes, updated at the cut: 1.
// 2: Phone empty (C 0) a year old (A 0.5): 0.075/0.55; Amount C 1, S 0:
//    0.40/0.55; Type S 0: 0.
// 3: Phone C 1, no update date (A leaves); Amount empty (C 0, X1 does not
//    apply); Type nothing applies, leaves.
const PHONE: [f64; 3] = [1.0, 0.075 / 0.55, 1.0];
const AMOUNT: [f64; 3] = [1.0, 0.40 / 0.55, 0.0];

#[test]
fn a_clients_icd_is_its_records_the_worst_for_a_critical_field() {
    let out = icd(MAPPING, "entities");
    let field = |f: &str| out.fields.iter().find(|(n, _)| n == f).unwrap().1;
    assert!(near(field("Phone"), PHONE.iter().sum::<f64>() / 3.0));
    assert!(near(field("Amount"), AMOUNT.iter().sum::<f64>() / 3.0));
    assert!(near(field("Type"), 0.5));
    assert!(out.fields.iter().all(|(n, _)| n != "Doc"), "no component");

    // `001.234` and `1234` are one client: Phone's worst, the others' mean.
    let one = (4.0 * PHONE[1] + 3.0 * (AMOUNT[0] + AMOUNT[1]) / 2.0 + 2.0 * 0.5) / 9.0;
    let other = 4.0 / 7.0;
    let keys: Vec<&str> = out.entities.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(keys, ["1234", "999"]);
    assert!(near(out.entities[0].icd, one));
    assert!(near(out.entities[1].icd, other));
    assert_eq!(out.entities[0].segment.as_deref(), Some("PN"));
    assert!(near(out.igcd, (one + other) / 2.0));
}

#[test]
fn without_an_entity_the_igcd_is_the_records_mean() {
    let out = icd(&MAPPING.replace(", roles: [entity]", ""), "records");
    assert!(out.entities.is_empty());
    let records = [
        1.0,
        (4.0 * PHONE[1] + 3.0 * AMOUNT[1]) / 9.0,
        (4.0 * PHONE[2] + 3.0 * AMOUNT[2]) / 7.0,
    ];
    assert!(near(out.igcd, records.iter().sum::<f64>() / 3.0));
}

#[test]
fn bands_are_the_guides() {
    assert_eq!(
        [band(0.95), band(0.90), band(0.8), band(0.75), band(0.7)],
        ["high", "high", "medium", "medium", "low"]
    );
}

// Type and Phone as dimensions. Serious: P1 (blocking, Phone critical)
// fails record 2, A1 (major, Amount high) record 3; X1 is minor.
#[test]
fn each_group_has_its_records_serious_failures_and_mean_icd() {
    let mapping = MAPPING
        .replace("roles: [segment]", "roles: [segment, dimension]")
        .replace(
            "priority: critical,",
            "priority: critical, roles: [dimension],",
        );
    let out = icd(&mapping, "groups");
    let records = [
        1.0,
        (4.0 * PHONE[1] + 3.0 * AMOUNT[1] + 2.0 * 0.0) / 9.0,
        (4.0 * PHONE[2] + 3.0 * AMOUNT[2]) / 7.0,
    ];
    let seen: Vec<(&str, Option<&str>, i64, i64)> = out
        .groups
        .iter()
        .map(|g| {
            (
                g.dimension.as_str(),
                g.group.as_deref(),
                g.records,
                g.serious,
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            ("Phone", Some("300"), 1, 0),
            ("Phone", Some("310"), 1, 1),
            ("Phone", None, 1, 1),
            ("Type", Some("PN"), 3, 2),
        ]
    );
    assert!(near(out.groups[2].icd, records[1]));
    assert!(near(out.groups[3].icd, records.iter().sum::<f64>() / 3.0));
}
