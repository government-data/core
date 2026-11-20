//! The rules' YAML as decisions 0019 and 0033 write it. Synthetic rules.

use quality_rules_model::{parse, Position};
use quality_rules_model::{Fields, OneOrMany, Pattern, Severity, Text, ValueSet};

const DATASET: &str = r##"dataset: clients

populations:
  individuals: client_type = 'PN'
  companies: client_type = 'PJ'

lists:
  income_activities: [ASALARIADO, INDEPENDIENTE AGRO, JUBILADO/PENSIONADO]
  codes: [08, 8430, 007]

placeholders:
  text: [ACTUALIZAR, SIN INFORMACION]
  date: ["1900-01-01"]
  email: ["notiene@*", "sincorreo@*"]

derived:
  age: years_between(FechaNacimientoCliente, :cut)

references:
  advisors: { dataset: advisors, key: CodigoAsesor, as_of: same_cut }

tables:
  activity_ciiu:
    when: ActividadLaboral
    then: CodigoCiiu
    rows:
      - { when: JUBILADO/PENSIONADO, allowed: [8430] }
      - { when: [AMA DE CASA, ESTUDIANTE, MENOR], allowed: [9820] }
      - { when: "*", forbidden: [8430, 9700, 9820] }

rules:
  - id: R-11
    field: NombreEmpresa
    check: required
    when: ActividadLaboral = 'EMPLEADO'
    population: individuals
    severity: major
    cause: capture
    alert: Empresa vacía para un empleado
    origin: { process: KNIME, node: "#1636" }
    examples:
      fails: { ActividadLaboral: EMPLEADO, NombreEmpresa: "" }
      passes: { ActividadLaboral: EMPLEADO, NombreEmpresa: ACME SAS }
      skips: [{ ActividadLaboral: PENSIONADO, NombreEmpresa: null }]

  - id: R-12
    field: [NombreRepresentanteLegal, NitRepresentanteLegal]
    check: required
    population: companies

  - id: R-20
    field: FechaExpedicion
    check: range
    min: FechaNacimiento + INTERVAL 18 YEAR
    max: :cut

  - id: R-21
    field: CodigoCiiu
    check: domain
    in: { list: codes }

  - id: R-40
    check: combination
    table: activity_ciiu
"##;

#[test]
fn reads_what_a_dataset_declares_once() {
    let file = parse(DATASET).unwrap();
    assert_eq!(file.dataset.value, "clients");
    assert_eq!(file.populations["individuals"].value, "client_type = 'PN'");
    assert_eq!(file.placeholders.email.len(), 2);
    assert_eq!(
        file.derived["age"].value,
        "years_between(FechaNacimientoCliente, :cut)"
    );
    assert_eq!(file.references["advisors"].key.value, "CodigoAsesor");
}

#[test]
fn keeps_codes_as_written() {
    let file = parse(DATASET).unwrap();
    let codes: Vec<&str> = file.lists["codes"]
        .value
        .iter()
        .map(|t| t.0.as_str())
        .collect();
    assert_eq!(codes, ["08", "8430", "007"]);
}

#[test]
fn reads_a_combination_table() {
    let table = &parse(DATASET).unwrap().tables["activity_ciiu"];
    assert_eq!(table.then.value, "CodigoCiiu");
    assert_eq!(table.rows.len(), 3);
    assert_eq!(
        table.rows[1].value.when,
        OneOrMany::Many(vec![
            Text("AMA DE CASA".into()),
            Text("ESTUDIANTE".into()),
            Text("MENOR".into())
        ])
    );
    assert_eq!(table.rows[2].value.forbidden.len(), 3);
}

#[test]
fn reads_rules_by_pattern_with_their_parameters() {
    let rules = parse(DATASET).unwrap().rules;
    assert_eq!(rules[0].check().value, Pattern::Required);
    assert_eq!(rules[0].severity.as_ref().unwrap().value, Severity::Major);
    assert_eq!(
        rules[0].examples.as_ref().unwrap().value.skips.all()[0]["NombreEmpresa"],
        None
    );
    assert_eq!(
        rules[1].field.as_ref().unwrap().value,
        Fields::Many(vec![
            "NombreRepresentanteLegal".into(),
            "NitRepresentanteLegal".into()
        ])
    );
    assert_eq!(rules[2].max.as_ref().unwrap().value.0, ":cut");
    assert_eq!(
        rules[3].in_.as_ref().unwrap().value,
        ValueSet::List {
            list: "codes".into()
        }
    );
    assert_eq!(rules[4].table.as_ref().unwrap().value, "activity_ciiu");
}

#[test]
fn places_every_value() {
    let rules = parse(DATASET).unwrap().rules;
    assert_eq!(
        rules[0].id.at,
        Position {
            line: 32,
            column: 9
        }
    );
    assert_eq!(rules[2].min.as_ref().unwrap().at.line, 54);
}

#[test]
fn places_what_is_wrong() {
    let unknown = parse("dataset: c\nrules:\n  - id: R-1\n    feild: Edad\n    check: required\n")
        .unwrap_err();
    assert_eq!(unknown.at, Some(Position { line: 4, column: 5 }));
    assert!(
        unknown.message.contains("unknown field `feild`"),
        "{unknown}"
    );

    let pattern = parse("dataset: c\nrules:\n  - id: R-1\n    check: rango\n").unwrap_err();
    assert_eq!(pattern.at.map(|p| p.line), Some(4));
    assert!(
        pattern.message.contains("unknown variant `rango`"),
        "{pattern}"
    );

    let duplicate =
        parse("dataset: c\nrules:\n  - id: R-1\n    check: required\n    check: empty\n")
            .unwrap_err();
    assert_eq!(duplicate.at.map(|p| p.line), Some(5));
}

#[test]
fn refuses_an_alias_bomb() {
    let mut yaml = String::from("a0: &a0 [x, x]\n");
    for i in 1..30 {
        yaml.push_str(&format!("a{i}: &a{i} [*a{p}, *a{p}]\n", p = i - 1));
    }
    yaml.push_str("dataset: c\nrules: []\n");
    assert!(parse(&yaml).is_err());
}

/// The schema editors use is generated from these types: regenerate it with
/// `UPDATE_SCHEMA=1 cargo test -p quality-rules-model`.
#[test]
fn the_published_schema_is_current() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/rules.schema.json");
    let current = serde_json::to_string_pretty(&quality_rules_model::json_schema()).unwrap() + "\n";
    if std::env::var_os("UPDATE_SCHEMA").is_some() {
        std::fs::write(path, &current).unwrap();
    }
    let published = std::fs::read_to_string(path).unwrap_or_default();
    assert!(
        published == current,
        "rules.schema.json is stale: UPDATE_SCHEMA=1 cargo test -p quality-rules-model"
    );
}

#[test]
fn values_of_every_shape_stay_as_written() {
    let yaml = "dataset: c\nrules:\n  - id: R-1\n    field: Codigo\n    check: domain\n    in: [08, NO, 1e3]\n    examples:\n      fails: { Codigo: 07, Monto: 100 }\n      passes: [{ Codigo: 08 }]\n";
    let rule = &parse(yaml).unwrap().rules[0];
    let ValueSet::Values(values) = &rule.in_.as_ref().unwrap().value else {
        panic!("values")
    };
    assert_eq!(
        values.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(),
        ["08", "NO", "1e3"]
    );
    let examples = &rule.examples.as_ref().unwrap().value;
    assert_eq!(examples.fails.all()[0]["Codigo"], Some(Text("07".into())));
    assert_eq!(examples.fails.all()[0]["Monto"], Some(Text("100".into())));
    assert_eq!(examples.passes.all()[0]["Codigo"], Some(Text("08".into())));
}

#[test]
fn refuses_a_lone_value_yaml_would_change() {
    let table = "dataset: c\ntables:\n  t:\n    when: A\n    then: B\n    rows:\n      - { when: NO, allowed: [x] }\nrules: []\n";
    let e = parse(table).unwrap_err();
    assert!(e.message.contains("write it in quotes"), "{e}");
    assert_eq!(e.at.map(|p| p.line), Some(7));
    let quoted = table.replace("when: NO", "when: \"NO\"");
    assert!(parse(&quoted).is_ok());
    let bad = parse(
        "dataset: c\nrules:\n  - { id: R, field: F, check: domain, in: { list: a, extra: b } }\n",
    );
    assert!(bad.is_err());
}
