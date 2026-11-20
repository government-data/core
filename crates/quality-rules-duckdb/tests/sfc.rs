//! The SFC indicator's cells (decision 0058) on six synthetic clients,
//! counted by hand: a cell is right when filled and failing no rule that
//! reads it, among the records whose `active` field is active.

use duckdb::Connection;
use quality::mapping::Mapping;
use quality::worker_api::SfcCell;
use quality_rules_duckdb::{IcdRule, Runner};
use quality_rules_lint::lint;

const MAPPING: &str = "dataset: clients
source: clients
fields:
  Doc: { column: doc, type: text }
  State: { column: state, type: text, roles: [active], active_values: [A, Activo] }
  Phone: { column: phone, type: text, roles: [sfc] }
  Email: { column: email, type: text, roles: [sfc] }
  City: { column: city, type: text, roles: [sfc] }
";

// Phone has three digits; Email is not the marker `bad`. City has no rule.
const RULES: &str = "dataset: clients
rules:
  - id: P1
    check: expression
    fails: length(Phone) <> 3
    dimension: validity
    severity: minor
    alert: a

  - id: E1
    check: expression
    fails: Email = 'bad'
    dimension: validity
    severity: major
    alert: a
";

// 1 active, all right; 2 active (` a `), Phone fails P1, City blank;
// 3 inactive; 4 active (`ACTIVO`), Email fails E1, City only U+00A0;
// 5 inactive (no state); 6 active, Phone empty.
fn clients(name: &str) -> (std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("qd-sfc-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("clients.parquet").display().to_string();
    Connection::open_in_memory()
        .unwrap()
        .execute_batch(&format!(
            "COPY (SELECT * FROM (VALUES
                ('1', 'A', '300', 'a@x', 'X'),
                ('2', ' a ', '30', 'a@x', ' '),
                ('3', 'I', '300', 'bad', 'X'),
                ('4', 'ACTIVO', '300', 'bad', chr(160)),
                ('5', NULL, '300', 'a@x', 'X'),
                ('6', 'A', NULL, 'a@x', 'Y')
              ) AS t(doc, state, phone, email, city)) TO '{path}' (FORMAT parquet)"
        ))
        .unwrap();
    (dir, path)
}

fn cells(mapping: &str, rules: &str, name: &str) -> Vec<SfcCell> {
    let (dir, path) = clients(name);
    let mapping = Mapping::parse(mapping).unwrap();
    let mut runner = Runner::new().unwrap();
    runner.load_mapped(&mapping, &path).unwrap();
    let report = lint(rules, &runner.schema("clients", &[]).unwrap());
    assert_eq!(report.findings, []);
    let file = quality_rules_model::parse(rules).unwrap();
    let tables = runner.tables(&file);
    let compiled: Vec<IcdRule> = report
        .compiled
        .iter()
        .map(|r| IcdRule::of(r, &tables).unwrap())
        .collect();
    let out = runner
        .icd("clients", &mapping, &compiled, "2026-12-31")
        .unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    out.cells
}

fn counts(cells: &[SfcCell]) -> Vec<(&str, i64, i64)> {
    cells
        .iter()
        .map(|c| (c.field.as_str(), c.ok, c.total))
        .collect()
}

#[test]
fn a_cell_counts_the_active_records_where_it_is_right() {
    let out = cells(MAPPING, RULES, "active");
    // Active: 1, 2, 4, 6. Phone right in 1, 4; Email in 1, 2, 6; City in
    // 1, 6. The mapping's order.
    assert_eq!(
        counts(&out),
        [("City", 2, 4), ("Email", 3, 4), ("Phone", 2, 4)]
    );
}

#[test]
fn without_an_active_field_every_record_counts() {
    let mapping = MAPPING.replace(", roles: [active], active_values: [A, Activo]", "");
    let out = cells(&mapping, RULES, "all");
    // Phone right in 1, 3, 4, 5; Email in 1, 2, 5, 6; City in 1, 3, 5, 6.
    assert_eq!(
        counts(&out),
        [("City", 4, 6), ("Email", 4, 6), ("Phone", 4, 6)]
    );
}

#[test]
fn without_rules_a_cell_is_right_when_filled() {
    let out = cells(MAPPING, "dataset: clients\nrules: []\n", "filled");
    // Among 1, 2, 4, 6: Phone filled in 1, 2, 4; Email in all; City in 1, 6.
    assert_eq!(
        counts(&out),
        [("City", 2, 4), ("Email", 4, 4), ("Phone", 3, 4)]
    );
    let none = cells(&MAPPING.replace("roles: [sfc]", "roles: []"), RULES, "none");
    assert!(none.is_empty());
}
