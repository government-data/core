//! A rules file linted with its table's global rules (decision 0045):
//! extended rules compile as written, what does not merge is an error at
//! its place, a key repeated says nothing. Synthetic rules.

use quality_rules_compile::{FieldType, Schema};
use quality_rules_lint::{lint, lint_globals, lint_with, Code, Severity};

fn schema() -> Schema {
    let mut s = Schema::default();
    for name in ["Document", "Representative", "Kind", "Ciiu"] {
        s.fields.insert(name.into(), FieldType::Text);
    }
    s
}

const GLOBALS: &str = r#"dataset: clients
lists:
  ciiu: ["0111", "8430"]
named:
  valid_document:
    check: format
    pattern: \d{5,12}
    severity: major
    cause: capture
    alert: Between 5 and 12 digits
"#;

fn codes(yaml: &str) -> Vec<(Code, u64)> {
    lint_with(yaml, &schema(), Some(GLOBALS))
        .findings
        .iter()
        .map(|f| (f.code, f.at.map_or(0, |a| a.line)))
        .collect()
}

#[test]
fn an_extended_rule_compiles_as_the_named_rule_with_its_changes() {
    let yaml = "dataset: clients\nrules:\n  - id: Document-01\n    field: Document\n    extends: valid_document\n\n  - id: Ciiu-01\n    field: Ciiu\n    check: domain\n    in: { list: ciiu }\n    severity: minor\n    cause: capture\n    alert: Not in CIIU\n";
    let report = lint_with(yaml, &schema(), Some(GLOBALS));
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    assert_eq!(
        report.compiled.len(),
        2,
        "the global list is the file's to use"
    );
    let extended = &report.extends[0];
    assert_eq!(
        (
            extended.rule.as_str(),
            extended.named.as_str(),
            extended.at.line
        ),
        ("Document-01", "valid_document", 5)
    );
}

#[test]
fn what_does_not_merge_is_an_error_where_it_is_written() {
    let unknown = "dataset: clients\nrules:\n  - id: Document-01\n    field: Document\n    extends: nowhere\n";
    assert_eq!(codes(unknown), [(Code::UnknownNamed, 5)]);
    let twice = "dataset: clients\nlists:\n  ciiu: [\"1\"]\nrules:\n  - id: Ciiu-01\n    field: Ciiu\n    check: domain\n    in: { list: ciiu }\n    severity: minor\n    cause: capture\n    alert: x\n";
    assert_eq!(codes(twice), [(Code::DeclaredTwice, 3)]);
    assert_eq!(Code::DeclaredTwice.severity(), Severity::Error);
    // Without the global rules, an extended rule has no pattern.
    let alone = lint(unknown, &schema());
    assert!(alone.blocks(), "{:?}", alone.findings);
}

#[test]
fn a_key_repeated_from_the_named_rule_says_nothing() {
    let yaml = "dataset: clients\nrules:\n  - id: Document-01\n    field: Document\n    extends: valid_document\n    severity: major\n    alert: Another alert\n";
    assert_eq!(codes(yaml), [(Code::RepeatsNamed, 6)]);
    assert_eq!(Code::RepeatsNamed.severity(), Severity::Warning);
}

#[test]
fn the_global_rules_have_no_rules_of_their_own() {
    let yaml = format!("{GLOBALS}rules:\n  - id: G-01\n    field: Kind\n    check: required\n");
    let report = lint_globals(&yaml, &schema());
    assert_eq!(report.findings[0].code, Code::RulesInGlobals);
    let alone = lint_globals(GLOBALS, &schema()).findings;
    assert!(alone.is_empty(), "{alone:?}");
}
