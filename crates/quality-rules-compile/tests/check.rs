//! Each pattern lowered with the semantics of decision 0033, point 4.
//! Synthetic fields and rules.

use quality_rules_compile::{compile, Compiled, FieldType, Problem, ProblemKind, Schema};
use quality_rules_model::parse;
use quality_rules_model::{Dimension, Pattern, Severity};

fn schema() -> Schema {
    let mut s = Schema::default();
    for (name, t) in [
        ("client_type", FieldType::Text),
        ("ActividadLaboral", FieldType::Text),
        ("NombreEmpresa", FieldType::Text),
        ("CodigoCiiu", FieldType::Text),
        ("Celular", FieldType::Text),
        ("Correo", FieldType::Email),
        ("Documento", FieldType::Text),
        ("DocumentoRef1", FieldType::Text),
        ("DocumentoRef2", FieldType::Text),
        ("CodigoAsesor", FieldType::Text),
        ("FechaNacimiento", FieldType::Date),
        ("FechaExpedicion", FieldType::Date),
        ("FechaActualizacion", FieldType::Date),
        ("Activo", FieldType::Number),
        ("Pasivo", FieldType::Number),
        ("Patrimonio", FieldType::Number),
        ("EsPep", FieldType::Text),
        ("CargoPep", FieldType::Text),
    ] {
        s.fields.insert(name.into(), t);
    }
    s
}

const HEAD: &str = r#"dataset: clients
populations:
  individuals: client_type = 'PN'
lists:
  no_income: [AMA DE CASA, ESTUDIANTE]
placeholders:
  text: [ACTUALIZAR]
  date: ["1900-01-01"]
  email: ["notiene@*"]
derived:
  age: years_between(FechaNacimiento, :cut)
references:
  advisors: { dataset: advisors, key: CodigoAsesor }
tables:
  activity_ciiu:
    when: ActividadLaboral
    then: CodigoCiiu
    rows:
      - { when: JUBILADO/PENSIONADO, allowed: ["8430"] }
      - { when: "*", forbidden: ["8430"] }
rules:
"#;

fn run(rules: &str) -> (Vec<Compiled>, Vec<Problem>) {
    let file = parse(&format!("{HEAD}{rules}")).unwrap();
    compile(&file, &schema())
}

fn one(rule: &str) -> Compiled {
    let (compiled, problems) = run(rule);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(compiled.len(), 1);
    compiled.into_iter().next().unwrap()
}

fn kinds(rule: &str) -> Vec<ProblemKind> {
    run(rule).1.into_iter().map(|p| p.kind).collect()
}

#[test]
fn required_counts_fillers_as_empty_and_never_skips() {
    let c = one("  - { id: R-1, field: NombreEmpresa, check: required, when: \"ActividadLaboral = 'EMPLEADO'\", population: individuals }\n");
    assert_eq!(
        c.fails.to_string(),
        "(is_empty(NombreEmpresa) OR upper(trim(NombreEmpresa)) IN (upper(trim('ACTUALIZAR'))))"
    );
    assert_eq!(
        c.applies.to_string(),
        "((upper(trim(client_type)) = upper(trim('PN'))) AND (upper(trim(ActividadLaboral)) = upper(trim('EMPLEADO'))))"
    );
    assert_eq!(c.skips, None);
    assert_eq!(
        (c.dimension, c.severity),
        (Dimension::Completeness, Severity::Blocking)
    );
}

#[test]
fn other_patterns_skip_an_empty_value() {
    let c = one("  - { id: R-2, field: CodigoCiiu, check: domain, in: [\"8430\", \"9700\"] }\n");
    assert_eq!(
        c.fails.to_string(),
        "upper(trim(CodigoCiiu)) NOT IN (upper(trim('8430')), upper(trim('9700')))"
    );
    assert_eq!(c.skips.unwrap().to_string(), "is_empty(CodigoCiiu)");
    let fail =
        one("  - { id: R-3, field: CodigoCiiu, check: domain, in: [\"8430\"], on_empty: fail }\n");
    assert!(fail.skips.is_none());
    assert!(fail
        .fails
        .to_string()
        .starts_with("(is_empty(CodigoCiiu) OR "));
}

#[test]
fn the_result_has_three_values() {
    let c = one("  - { id: R-2, field: CodigoCiiu, check: domain, not_in: { list: no_income } }\n");
    assert_eq!(
        c.result().to_string(),
        format!("CASE WHEN NOT (coalesce(TRUE, FALSE)) THEN NULL WHEN coalesce(is_empty(CodigoCiiu), FALSE) THEN NULL WHEN {f} THEN TRUE WHEN NOT ({f}) THEN FALSE END", f = c.fails)
    );
}

#[test]
fn exact_case_keeps_the_comparison_as_written() {
    let c = one("  - { id: R-4, field: NombreEmpresa, check: required, when: \"ActividadLaboral = 'EMPLEADO'\", case: exact }\n");
    assert_eq!(c.applies.to_string(), "(ActividadLaboral = 'EMPLEADO')");
}

#[test]
fn lists_and_derived_fields_expand() {
    let c = one("  - { id: R-5, field: NombreEmpresa, check: empty, when: \"in_list(ActividadLaboral, 'no_income') AND age < 18\" }\n");
    assert_eq!(
        c.applies.to_string(),
        "(upper(trim(ActividadLaboral)) IN (upper(trim('AMA DE CASA')), upper(trim('ESTUDIANTE'))) AND (years_between(FechaNacimiento, :cut) < 18))"
    );
    assert_eq!(c.fails.to_string(), "NOT (is_empty(NombreEmpresa))");
}

#[test]
fn fillers_by_type_and_pattern() {
    let c = one("  - { id: R-6, field: Correo, check: placeholder }\n");
    assert_eq!(c.fails.to_string(), "(upper(trim(Correo)) IN (upper(trim('ACTUALIZAR'))) OR upper(trim(Correo)) LIKE 'NOTIENE@%')");
    let d = one("  - { id: R-7, field: FechaNacimiento, check: placeholder }\n");
    assert_eq!(
        d.fails.to_string(),
        "FechaNacimiento IN (DATE '1900-01-01')"
    );
}

#[test]
fn length_format_range_fresh() {
    assert_eq!(
        one("  - { id: L, field: Celular, check: length, in: [10] }\n")
            .fails
            .to_string(),
        "length(trim(Celular)) NOT IN (10)"
    );
    assert_eq!(
        one("  - { id: F, field: Celular, check: format, pattern: '3\\d{9}' }\n")
            .fails
            .to_string(),
        "NOT (regexp_like(trim(Celular), '3\\d{9}'))"
    );
    assert_eq!(
        one("  - { id: N, field: Celular, check: format, named: mobile_co }\n")
            .fails
            .to_string(),
        "NOT (matches(trim(Celular), MobileCo))"
    );
    let r = one("  - { id: R, field: FechaExpedicion, check: range, min: FechaNacimiento + INTERVAL 18 YEAR, max: ':cut', max_exclusive: true }\n");
    assert_eq!(
        r.fails.to_string(),
        "((FechaExpedicion < (FechaNacimiento + INTERVAL 18 YEAR)) OR (FechaExpedicion >= :cut))"
    );
    let d = one("  - { id: D, field: FechaExpedicion, check: range, min: 1950-01-01 }\n");
    assert_eq!(d.fails.to_string(), "(FechaExpedicion < DATE '1950-01-01')");
    let f = one("  - { id: T, field: FechaActualizacion, check: fresh, max_age: 13 months }\n");
    assert_eq!(
        f.fails.to_string(),
        "(FechaActualizacion < (:cut - INTERVAL 13 MONTH))"
    );
}

#[test]
fn sum_unique_differs() {
    let s = one(
        "  - { id: S, field: Patrimonio, check: sum, parts: [Activo, '- Pasivo'], tolerance: 1 }\n",
    );
    assert_eq!(
        s.fails.to_string(),
        "(abs((Patrimonio - (Activo + -(Pasivo)))) > 1)"
    );
    let u = one("  - { id: U, field: Documento, check: unique, normalize: true }\n");
    assert_eq!(u.fails.to_string(), "duplicated(normalized(Documento))");
    assert_eq!(u.dimension, Dimension::Uniqueness);
    let d = one("  - { id: D, field: DocumentoRef1, check: differs, from: Documento }\n");
    assert_eq!(
        d.fails.to_string(),
        "(upper(trim(DocumentoRef1)) = upper(trim(Documento)))"
    );
    assert_eq!(
        d.skips.unwrap().to_string(),
        "(is_empty(DocumentoRef1) OR is_empty(Documento))"
    );
}

#[test]
fn combination_fails_once_whichever_row() {
    let c = one("  - { id: C, check: combination, table: activity_ciiu }\n");
    assert_eq!(c.field.as_deref(), Some("CodigoCiiu"));
    assert_eq!(
        c.fails.to_string(),
        "((upper(trim(ActividadLaboral)) IN (upper(trim('JUBILADO/PENSIONADO'))) AND upper(trim(CodigoCiiu)) NOT IN (upper(trim('8430')))) \
         OR (upper(trim(ActividadLaboral)) NOT IN (upper(trim('JUBILADO/PENSIONADO'))) AND upper(trim(CodigoCiiu)) IN (upper(trim('8430')))))"
    );
}

#[test]
fn implies_both_ways_and_expression() {
    let i = one("  - { id: I, check: implies, if: \"EsPep = 'SI'\", then: not is_empty(CargoPep), both_ways: true }\n");
    assert!(i.fails.to_string().contains(" OR "));
    assert_eq!(i.pattern, Pattern::Implies);
    let e = one("  - { id: E, check: expression, fails: Activo < 0, dimension: accuracy }\n");
    assert_eq!(
        (e.fails.to_string(), e.dimension),
        ("(Activo < 0)".into(), Dimension::Accuracy)
    );
}

#[test]
fn a_rule_over_several_fields_or_values_expands() {
    let (c, p) = run("  - { id: R-8, field: [NombreEmpresa, Celular], check: required }\n  - { id: R-9, field: 'DocumentoRef{n}', check: differs, from: Documento, for_each: { n: [1, 2] } }\n");
    assert!(p.is_empty(), "{p:?}");
    let ids: Vec<&str> = c.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["R-8.NombreEmpresa", "R-8.Celular", "R-9.1", "R-9.2"]);
    assert_eq!(c[3].field.as_deref(), Some("DocumentoRef2"));
}

#[test]
fn reports_what_keeps_a_rule_from_compiling() {
    assert_eq!(
        kinds("  - { id: A, field: Nada, check: required }\n"),
        [ProblemKind::UnknownField("Nada".into())]
    );
    assert_eq!(
        kinds("  - { id: B, field: Celular, check: required, min: 3 }\n"),
        [ProblemKind::NotApplicable("min")]
    );
    assert_eq!(
        kinds("  - { id: C, field: Celular, check: format }\n"),
        [ProblemKind::Missing("pattern")]
    );
    assert_eq!(
        kinds("  - { id: D, field: Celular, check: required, population: nobody }\n"),
        [ProblemKind::UnknownPopulation("nobody".into())]
    );
    assert_eq!(
        kinds("  - { id: E, check: combination, table: none }\n"),
        [ProblemKind::UnknownTable("none".into())]
    );
    assert_eq!(
        kinds("  - { id: F, field: Activo, check: domain, in: [abc] }\n"),
        [ProblemKind::BadValue {
            parameter: "in",
            value: "abc".into()
        }]
    );
    assert_eq!(
        kinds("  - { id: G, field: FechaActualizacion, check: fresh, max_age: soon }\n"),
        [ProblemKind::BadValue {
            parameter: "max_age",
            value: "soon".into()
        }]
    );
    assert_eq!(
        kinds("  - { id: H, check: expression, fails: Activo < 0 }\n"),
        [ProblemKind::Missing("dimension")]
    );
    assert_eq!(
        kinds(
            "  - { id: I, field: CodigoAsesor, check: domain, in: { reference: others.Codigo } }\n"
        ),
        [ProblemKind::UnknownReference("others.Codigo".into())]
    );
    assert!(matches!(
        kinds("  - { id: J, field: Celular, check: required, when: (SELECT 1) }\n")[..],
        [ProblemKind::Expression(_)]
    ));
    let (_, p) = run("  - { id: K, field: Celular, check: required, min: 3 }\n");
    assert_eq!((p[0].rule.as_str(), p[0].at.line), ("K", 22));
}

#[test]
fn a_domain_can_point_at_a_reference() {
    let c = one("  - { id: R, field: CodigoAsesor, check: domain, in: { reference: advisors.CodigoAsesor } }\n");
    assert_eq!(
        c.fails.to_string(),
        "NOT (in_reference(CodigoAsesor, 'advisors.CodigoAsesor'))"
    );
}

#[test]
fn a_derived_field_reads_fields_of_the_dataset() {
    let file = parse("dataset: c\nderived:\n  edad: years_between(Nacimiento, :cut)\nrules:\n  - { id: R, field: edad, check: range, min: 0 }\n").unwrap();
    let (compiled, problems) = compile(&file, &schema());
    assert!(compiled.is_empty());
    let kinds: Vec<_> = problems
        .iter()
        .map(|p| (p.rule.as_str(), p.kind.clone()))
        .collect();
    assert_eq!(
        kinds,
        [
            (
                "derived.edad",
                ProblemKind::UnknownField("Nacimiento".into())
            ),
            ("R", ProblemKind::UnknownField("edad".into()))
        ]
    );
}

#[test]
fn references_are_declared_and_their_filters_parse() {
    let file = parse("dataset: c\nreferences:\n  r: { dataset: d, key: K, where: \"Estado = 'A' ;\" }\nrules:\n  - { id: E, check: expression, dimension: validity, fails: \"in_reference(CodigoAsesor, 'otra.K')\" }\n").unwrap();
    let (_, problems) = compile(&file, &schema());
    let kinds: Vec<_> = problems
        .iter()
        .map(|p| (p.rule.as_str(), p.kind.clone()))
        .collect();
    assert!(
        matches!(kinds[0], ("references.r", ProblemKind::Expression(_))),
        "{kinds:?}"
    );
    assert_eq!(
        kinds[1],
        ("E", ProblemKind::UnknownReference("otra.K".into()))
    );
}

#[test]
fn a_derived_case_takes_the_type_of_its_branches() {
    let yaml = "dataset: c\nderived:\n  total: CASE WHEN client_type = 'PN' THEN Activo + Pasivo ELSE Patrimonio END\n  nombre: CASE WHEN client_type = 'PN' THEN NombreEmpresa END\nrules:\n  - { id: T, field: total, check: range, min: 0 }\n  - { id: N, field: nombre, check: domain, in: [ACME] }\n";
    let (compiled, problems) = compile(&parse(yaml).unwrap(), &schema());
    assert!(problems.is_empty(), "{problems:?}");
    assert!(
        compiled[0].fails.to_string().starts_with("(CASE WHEN"),
        "{}",
        compiled[0].fails
    );
    assert!(
        compiled[1].fails.to_string().starts_with("upper(trim(CASE"),
        "{}",
        compiled[1].fails
    );
}

#[test]
fn a_rule_reads_its_field_its_condition_and_a_derived_fields_sources() {
    let r = one("  - { id: R-1, field: NombreEmpresa, check: required, when: \"ActividadLaboral = 'EMPLEADO'\", population: individuals }\n");
    let mut read = r.reads();
    read.sort();
    assert_eq!(read, ["ActividadLaboral", "NombreEmpresa", "client_type"]);
    let e = one(
        "  - { id: E, check: expression, fails: age < 18 AND Activo < 0, dimension: validity }\n",
    );
    let mut read = e.reads();
    read.sort();
    assert_eq!(
        read,
        ["Activo", "FechaNacimiento"],
        "age is read as what it is computed from"
    );
}
