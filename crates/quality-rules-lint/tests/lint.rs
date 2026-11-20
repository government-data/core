//! Lint on synthetic rules: each check finds what it should, at its place,
//! and the rules' examples run on an embedded DuckDB.

use duckdb::types::Value;
use duckdb::Connection;
use quality_rules_compile::{FieldType, Schema};
use quality_rules_lint::{check_examples, draft, lint, Code, ExampleQuery, Finding, Severity};

fn schema() -> Schema {
    let mut s = Schema::default();
    for (name, t) in [
        ("ActividadLaboral", FieldType::Text),
        ("NombreEmpresa", FieldType::Text),
        ("Celular", FieldType::Text),
        ("Monto", FieldType::Number),
        ("Nacimiento", FieldType::Date),
    ] {
        s.fields.insert(name.into(), t);
    }
    s
}

fn findings(rules: &str) -> Vec<Finding> {
    lint(
        &format!("dataset: c\npopulations:\n  personas: ActividadLaboral = 'X'\nrules:\n{rules}"),
        &schema(),
    )
    .findings
}

fn codes(rules: &str) -> Vec<Code> {
    findings(rules).into_iter().map(|f| f.code).collect()
}

fn value<'a>(f: &'a Finding, name: &str) -> Option<&'a str> {
    f.values
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v.as_str())
}

const OK: &str = "alert: ok";

#[test]
fn a_clean_rule_has_no_findings() {
    assert_eq!(
        codes(&format!(
            "  - {{ id: R, field: Celular, check: format, named: mobile_co, {OK} }}\n"
        )),
        []
    );
}

#[test]
fn reports_what_keeps_a_rule_from_compiling_with_suggestions() {
    let f = findings(&format!(
        "  - {{ id: R, field: Celulr, check: required, {OK} }}\n"
    ));
    assert_eq!(
        (f[0].code, value(&f[0], "suggestion")),
        (Code::UnknownField, Some("Celular"))
    );
    assert_eq!(f[0].at.map(|p| p.line), Some(5));
    let p = findings(&format!(
        "  - {{ id: R, field: Celular, check: required, population: persona, {OK} }}\n"
    ));
    assert_eq!(
        (
            p[0].code,
            value(&p[0], "kind_key"),
            value(&p[0], "suggestion")
        ),
        (
            Code::UnknownName,
            Some("quality.lint.kind.population"),
            Some("personas")
        )
    );
    assert_eq!(
        codes(&format!(
            "  - {{ id: R, field: Celular, check: required, min: 3, {OK} }}\n"
        )),
        [Code::NotApplicable]
    );
    assert_eq!(
        codes(&format!(
            "  - {{ id: R, field: Celular, check: format, {OK} }}\n"
        )),
        [Code::Missing]
    );
    assert_eq!(
        codes(&format!(
            "  - {{ id: R, field: Monto, check: domain, in: [abc], {OK} }}\n"
        )),
        [Code::BadValue]
    );
    let e = findings(&format!(
        "  - {{ id: R, field: Celular, check: required, when: \"x = 1; DROP TABLE t\", {OK} }}\n"
    ));
    assert_eq!(
        (e[0].code, value(&e[0], "reason_key")),
        (Code::Expression, Some("quality.lint.reason.trailing_input"))
    );
    assert!(lint(
        "dataset: c\nrules:\n  - id: R\n    check: rango\n",
        &schema()
    )
    .blocks());
    assert_eq!(codes("  - id: [\n"), [Code::Syntax]);
}

#[test]
fn warns_about_what_compiles_but_is_suspect() {
    assert_eq!(
        codes("  - { id: R, field: Celular, check: required }\n"),
        [Code::NoAlert]
    );
    let both = codes(&format!("  - {{ id: A, field: Celular, check: required, {OK} }}\n\n  - {{ id: B, field: Celular, check: placeholder, values: [ACTUALIZAR], {OK} }}\n"));
    assert_eq!(both, [Code::RequiredAndPlaceholder]);
    let twice = codes(&format!("  - {{ id: A, field: Celular, check: required, {OK} }}\n\n  - {{ id: B, field: Celular, check: required, {OK} }}\n"));
    assert_eq!(twice, [Code::DuplicateRule]);
    let same_id = codes(&format!("  - {{ id: A, field: Celular, check: required, {OK} }}\n\n  - {{ id: A, field: NombreEmpresa, check: required, {OK} }}\n"));
    assert_eq!(same_id, [Code::DuplicateId]);
    let never = codes(&format!("  - {{ id: R, field: Celular, check: required, when: ActividadLaboral = 'A' AND ActividadLaboral = 'B', {OK} }}\n"));
    assert_eq!(never, [Code::NeverHolds]);
    assert_eq!(
        codes(&format!(
            "  - {{ id: R, field: Celular, check: format, pattern: (a)\\1, {OK} }}\n"
        )),
        [Code::Regex]
    );
    assert_eq!(
        codes(&format!(
            "  - {{ id: R, field: Monto, check: length, max: 10, {OK} }}\n"
        )),
        [Code::TextOnNumber]
    );
    // An expression is not judged by how many conditions it has (0052).
    let long = codes(&format!("  - id: R\n    check: expression\n    dimension: accuracy\n    fails: Monto < 0 OR Monto > 9 OR Celular = 'a' OR Celular = 'b'\n    {OK}\n"));
    assert_eq!(long, []);
    assert!(findings("  - { id: R, field: Celular, check: required }\n")
        .iter()
        .all(|f| f.severity() == Severity::Warning));
}

#[test]
fn applies_the_sizes_of_decision_0034() {
    let many: String = (0..31)
        .map(|i| {
            format!("\n  - {{ id: R{i}, field: Celular, check: format, pattern: '{i}', {OK} }}\n")
        })
        .collect();
    assert_eq!(codes(&many), [Code::LargeFile]);
    // One value a line: a list that long goes past 120 characters (0041).
    let values: String = (0..51).map(|i| format!("      - {i}\n")).collect();
    assert_eq!(
        codes(&format!(
            "  - id: R\n    field: Monto\n    check: domain\n    in:\n{values}    {OK}\n"
        )),
        [Code::LongList]
    );
    assert_eq!(draft(26).map(|f| f.code), Some(Code::LargeDraft));
    assert_eq!(draft(25), None);
}

/// What the caller does: run each query on an in-memory DuckDB.
fn run(queries: &[ExampleQuery]) -> Vec<Finding> {
    let con = Connection::open_in_memory().unwrap();
    queries
        .iter()
        .flat_map(|q| {
            let mut stmt = con
                .prepare(&q.sql)
                .unwrap_or_else(|e| panic!("{e}\n{}", q.sql));
            let cut = vec!["2026-10-31"; q.cut_params];
            let results: Vec<Option<bool>> = stmt
                .query_map(duckdb::params_from_iter(cut.iter()), |row| {
                    row.get::<_, Value>(0)
                })
                .unwrap()
                .map(|v| match v.unwrap() {
                    Value::Boolean(b) => Some(b),
                    _ => None,
                })
                .collect();
            check_examples(q, &results)
        })
        .collect()
}

const EXAMPLES: &str = "  - id: R
    field: NombreEmpresa
    check: required
    when: ActividadLaboral = 'EMPLEADO'
    alert: Empresa vacía
    examples:
      fails: { ActividadLaboral: EMPLEADO, NombreEmpresa: \"\" }
      passes: [{ ActividadLaboral: empleado, NombreEmpresa: ACME }]
      skips: { ActividadLaboral: PENSIONADO, NombreEmpresa: null }
";

#[test]
fn the_examples_run_on_duckdb() {
    let report = lint(&format!("dataset: c\nrules:\n{EXAMPLES}"), &schema());
    assert_eq!(report.findings, []);
    assert_eq!(report.examples.len(), 1);
    assert_eq!(run(&report.examples), []);

    let wrong = EXAMPLES.replace(
        "passes: [{ ActividadLaboral: empleado, NombreEmpresa: ACME }]",
        "passes: [{ ActividadLaboral: EMPLEADO, NombreEmpresa: ACTUALIZAR }]",
    );
    let found = run(&lint(
        &format!("dataset: c\nplaceholders:\n  text: [ACTUALIZAR]\nrules:\n{wrong}"),
        &schema(),
    )
    .examples);
    assert_eq!(found.len(), 1);
    assert_eq!(
        (
            value(&found[0], "expected_key"),
            value(&found[0], "index"),
            value(&found[0], "got_key")
        ),
        (
            Some("quality.lint.example.passes"),
            Some("1"),
            Some("quality.lint.example.fails")
        )
    );
}

#[test]
fn an_example_must_name_fields_of_the_dataset_with_values_of_their_type() {
    let unknown = EXAMPLES.replace("NombreEmpresa: ACME", "NombreEmpresa: ACME, Otro: x");
    assert_eq!(
        lint(&format!("dataset: c\nrules:\n{unknown}"), &schema())
            .findings
            .iter()
            .map(|f| f.code)
            .collect::<Vec<_>>(),
        [Code::UnknownField]
    );
    let typed = "  - { id: R, field: Monto, check: range, min: 0, alert: a, examples: { fails: { Monto: abc } } }\n";
    assert_eq!(codes(typed), [Code::BadValue]);
}

#[test]
fn a_regex_construct_inside_a_class_is_a_plain_character() {
    let ok = format!(
        "  - {{ id: R, field: Celular, check: format, pattern: '[a-z*+?]+@[(?=]x', {OK} }}\n"
    );
    assert_eq!(codes(&ok), []);
    assert_eq!(
        codes(&format!(
            "  - {{ id: R, field: Celular, check: format, pattern: 'a*+[x]', {OK} }}\n"
        )),
        [Code::Regex]
    );
}

#[test]
fn the_same_finding_at_the_same_place_is_said_once() {
    let twice = findings(&format!(
        "  - id: R\n    check: expression\n    dimension: accuracy\n    fails: is_empty(Nope) OR Nope = 0\n    {OK}\n"
    ));
    assert_eq!(
        twice
            .iter()
            .filter(|f| f.code == Code::UnknownField)
            .count(),
        1
    );
}
