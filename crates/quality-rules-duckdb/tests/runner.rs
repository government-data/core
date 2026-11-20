//! The runner on two synthetic datasets written as Parquet: the schema read
//! from the file, the rules' counts in one pass, a reference to the other
//! dataset, and the lint's examples.

use duckdb::Connection;
use quality_rules_compile::FieldType;
use quality_rules_duckdb::{fields_read, Counts, Outcome, Runner, Shown};
use quality_rules_lint::lint;

/// Two Parquet files in a fresh directory of the test `name`: clients and
/// advisors. Tests run at once: each its own directory.
fn parquet(name: &str) -> (std::path::PathBuf, String, String) {
    let dir = std::env::temp_dir().join(format!("qd-rules-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let clients = dir.join("clients.parquet").display().to_string();
    let advisors = dir.join("advisors.parquet").display().to_string();
    let con = Connection::open_in_memory().unwrap();
    con.execute_batch(&format!(
        "COPY (SELECT * FROM (VALUES
            ('1', 'EMPLEADO', 'ACME', '3001234567', DATE '1990-05-01', 'A1', 100.0),
            ('2', 'EMPLEADO', NULL, '300', DATE '2015-01-01', 'A9', 50.0),
            ('3', 'PENSIONADO', NULL, NULL, DATE '1950-01-01', 'A2', NULL),
            ('4', 'ACTUALIZAR', ' ', '3109876543', DATE '1980-12-31', NULL, 0.0)
          ) AS t(Documento, Actividad, Empresa, Celular, Nacimiento, Asesor, Monto)) TO '{clients}' (FORMAT parquet);
         COPY (SELECT * FROM (VALUES ('A1'), ('A2')) AS t(Codigo)) TO '{advisors}' (FORMAT parquet);"
    ))
    .unwrap();
    (dir, clients, advisors)
}

const RULES: &str = "dataset: clients
placeholders:
  text: [ACTUALIZAR]
references:
  advisors: { dataset: advisors, key: Codigo }
rules:
  - { id: R1, field: Empresa, check: required, when: Actividad = 'EMPLEADO', alert: a }

  - { id: R2, field: Celular, check: format, named: mobile_co, alert: a }

  - { id: R3, field: Asesor, check: domain, in: { reference: advisors.Codigo }, alert: a }

  - { id: R4, check: expression, fails: \"years_between(Nacimiento, :cut) < 18\", dimension: accuracy, alert: a,
      examples: { fails: { Nacimiento: 2020-01-01 }, passes: { Nacimiento: 1990-01-01 } } }
";

#[test]
fn reads_the_schema_and_counts_every_rule_in_one_pass() {
    let (dir, clients, advisors) = parquet("counts");
    let mut runner = Runner::new().unwrap();
    runner.load("clients", &clients).unwrap();
    runner.load("advisors", &advisors).unwrap();

    let schema = runner.schema("clients", &[]).unwrap();
    assert_eq!(schema.fields["Documento"], FieldType::Text);
    assert_eq!(schema.fields["Nacimiento"], FieldType::Date);
    assert_eq!(schema.fields["Monto"], FieldType::Number);

    let report = lint(RULES, &schema);
    assert_eq!(report.findings, []);
    assert_eq!(runner.examples(&report.examples, "2026-10-31").unwrap(), []);

    let file = quality_rules_model::parse(RULES).unwrap();
    let counts = runner
        .test(
            "clients",
            &report.compiled,
            &runner.tables(&file),
            "2026-10-31",
        )
        .unwrap();
    let c = |applies, fails, passes, skipped, rule: &str| Counts {
        rule: rule.into(),
        applies,
        fails,
        passes,
        skipped,
    };
    assert_eq!(
        counts,
        [
            c(2, 1, 1, 2, "R1"),
            c(3, 1, 2, 1, "R2"),
            c(3, 1, 2, 1, "R3"),
            c(4, 1, 3, 0, "R4")
        ]
    );
    assert_eq!(counts[0].conformity(), Some(50.0));
    std::fs::remove_dir_all(dir).unwrap();
}

/// A name with control characters is refused; a path stays one quoted
/// text (here a file that does not exist), never part of the statement.
#[test]
fn a_dataset_name_or_path_cannot_change_the_statement() {
    let mut runner = Runner::new().unwrap();
    assert!(runner.load("x\u{0}", "a.parquet").is_err());
    let e = runner
        .load("clients", "a.parquet'); DROP TABLE t; --")
        .unwrap_err()
        .to_string();
    assert!(e.contains("a.parquet'); DROP TABLE t; --"), "{e}");
}

/// Through a mapping, the rules name the dataset's fields, whatever the
/// source calls its columns; a number mapped as text reads without `.0`.
#[test]
fn reads_a_dataset_through_its_mapping() {
    let dir = std::env::temp_dir().join(format!("qd-rules-mapped-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("source.parquet").display().to_string();
    Connection::open_in_memory()
        .unwrap()
        .execute_batch(&format!(
            "COPY (SELECT * FROM (VALUES (8430.0, 'JUBILADO', TIMESTAMP '1950-01-01 00:00:00'), (4711.0, 'JUBILADO', NULL))
               AS t(Ciiu, ActLaboral, FecNac)) TO '{source}' (FORMAT parquet)"
        ))
        .unwrap();
    let mapping = quality::mapping::Mapping::parse(
        "dataset: clientes\nsource: fuente\nfields:\n  CodigoCiiu: { column: Ciiu, type: text, semantic: code }\n  ActividadLaboral: { column: ActLaboral, type: text }\n  Nacimiento: { column: FecNac, type: date }\n",
    )
    .unwrap();
    let mut runner = Runner::new().unwrap();
    runner.load_mapped(&mapping, &source).unwrap();
    let schema = runner.schema("clientes", &[]).unwrap();
    assert_eq!(
        (schema.fields["CodigoCiiu"], schema.fields["Nacimiento"]),
        (FieldType::Text, FieldType::Date)
    );
    let rules = "dataset: clientes\nrules:\n  - { id: C, field: CodigoCiiu, check: domain, in: [\"8430\"], when: ActividadLaboral = 'JUBILADO', alert: a }\n\n  - { id: N, field: Nacimiento, check: required, alert: a }\n";
    let report = lint(rules, &schema);
    assert_eq!(report.findings, []);
    let counts = runner
        .test(
            "clientes",
            &report.compiled,
            &Default::default(),
            "2026-10-31",
        )
        .unwrap();
    assert_eq!(
        (counts[0].passes, counts[0].fails, counts[1].fails),
        (1, 1, 1)
    );

    let wrong = quality::mapping::Mapping::parse(
        "dataset: d\nsource: fuente\nfields:\n  X: { column: NoExiste, type: text }\n",
    )
    .unwrap();
    assert!(
        matches!(runner.load_mapped(&wrong, &source), Err(quality_rules_duckdb::Error::InvalidColumnName(c)) if c == "NoExiste")
    );
    std::fs::remove_dir_all(dir).unwrap();
}

/// A reference with `where`: in the master, and among its active records.
/// The master has a NULL key, which must not turn a miss into a skip.
#[test]
fn a_reference_keeps_only_the_records_its_where_names() {
    let dir = std::env::temp_dir().join(format!("qd-rules-where-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (clients, advisors) = (
        dir.join("c.parquet").display().to_string(),
        dir.join("a.parquet").display().to_string(),
    );
    let con = Connection::open_in_memory().unwrap();
    con.execute_batch(&format!(
        "COPY (SELECT * FROM (VALUES ('A1'), ('A2'), ('A9')) AS t(Asesor)) TO '{clients}' (FORMAT parquet);
         COPY (SELECT * FROM (VALUES ('A1', 'ACTIVO'), ('A2', 'INACTIVO'), (NULL, 'ACTIVO')) AS t(Codigo, Estado)) TO '{advisors}' (FORMAT parquet);"
    ))
    .unwrap();
    let mut runner = Runner::new().unwrap();
    runner.load("clients", &clients).unwrap();
    runner.load("advisors", &advisors).unwrap();
    let rules = "dataset: clients
references:
  advisors: { dataset: advisors, key: Codigo }
  active: { dataset: advisors, key: Codigo, where: Estado = 'ACTIVO' }
rules:
  - { id: P29, field: Asesor, check: domain, in: { reference: advisors.Codigo }, alert: a }

  - { id: P30, check: expression, dimension: validity, alert: a,
      fails: \"in_reference(Asesor, 'advisors.Codigo') AND NOT in_reference(Asesor, 'active.Codigo')\" }
";
    let report = lint(rules, &runner.schema("clients", &[]).unwrap());
    assert_eq!(report.findings, []);
    let file = quality_rules_model::parse(rules).unwrap();
    let counts = runner
        .test(
            "clients",
            &report.compiled,
            &runner.tables(&file),
            "2026-10-31",
        )
        .unwrap();
    assert_eq!((counts[0].fails, counts[1].fails), (1, 1));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn reads_a_few_records_of_each_outcome_with_the_fields_a_rule_reads() {
    let (dir, clients, advisors) = parquet("sample");
    let mut runner = Runner::new().unwrap();
    runner.load("clients", &clients).unwrap();
    runner.load("advisors", &advisors).unwrap();
    let schema = runner.schema("clients", &[]).unwrap();
    let report = lint(RULES, &schema);
    let file = quality_rules_model::parse(RULES).unwrap();
    let tables = runner.tables(&file);
    let r1 = report.compiled.iter().find(|r| r.id == "R1").unwrap();
    let mut read = fields_read(r1);
    read.sort();
    assert_eq!(read, ["Actividad", "Empresa"]);

    let key = ["Documento".to_string()];
    let shown = Shown {
        key: &key,
        extra: &[],
    };
    let records = runner
        .sample("clients", r1, &tables, shown, "2026-10-31", 1)
        .unwrap();
    let outcomes: Vec<Outcome> = records.iter().map(|r| r.outcome).collect();
    assert_eq!(
        outcomes,
        [Outcome::Fails, Outcome::Passes, Outcome::Skips],
        "one of each"
    );
    let failing = &records[0].values;
    assert_eq!(
        failing[0],
        ("Documento".to_string(), Some("2".to_string())),
        "the key first"
    );
    assert!(failing.contains(&("Empresa".to_string(), None)));
    assert!(failing.contains(&("Actividad".to_string(), Some("EMPLEADO".to_string()))));

    let all = runner
        .sample("clients", r1, &tables, shown, "2026-10-31", 10)
        .unwrap();
    let skipped: Vec<&str> = all
        .iter()
        .filter(|r| r.outcome == Outcome::Skips)
        .map(|r| r.values[0].1.as_deref().unwrap())
        .collect();
    assert_eq!(skipped, ["3", "4"], "in the key's order");

    // Other fields asked for come after, each once.
    let extra = ["Celular".to_string(), "Empresa".to_string()];
    let more = runner
        .sample(
            "clients",
            r1,
            &tables,
            Shown {
                key: &key,
                extra: &extra,
            },
            "2026-10-31",
            1,
        )
        .unwrap();
    let names: Vec<&str> = more[0].values.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(
        names.len(),
        4,
        "the key, the two read and Celular: {names:?}"
    );
    assert_eq!(names[3], "Celular");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_rules_failures_are_grouped_by_the_shape_of_what_it_read() {
    let (dir, clients, advisors) = parquet("groups");
    let mut runner = Runner::new().unwrap();
    runner.load("clients", &clients).unwrap();
    runner.load("advisors", &advisors).unwrap();
    // Celular: a 3 and nine digits; the synthetic ones fail as '300' and empty.
    let rules = "dataset: clients\nrules:\n  - id: C1\n    field: Celular\n    check: format\n    pattern: '3\\d{9}'\n    severity: major\n    alert: Celular\n";
    let report = lint(rules, &runner.schema("clients", &[]).unwrap());
    let file = quality_rules_model::parse(rules).unwrap();
    let tables = runner.tables(&file);
    let key = ["Documento".to_string()];
    let shown = Shown {
        key: &key,
        extra: &[],
    };
    let groups = runner
        .fail_groups(
            "clients",
            &report.compiled[0],
            &tables,
            shown,
            "2026-10-31",
            (20, 5),
        )
        .unwrap();
    let shapes: Vec<(&str, u64)> = groups.iter().map(|g| (g.shape.as_str(), g.count)).collect();
    assert_eq!(
        shapes,
        [("999", 1)],
        "an empty value does not apply: no failure"
    );
    assert_eq!(groups[0].records.len(), 1);
    assert_eq!(
        groups[0].records[0][0],
        ("Documento".to_string(), Some("2".to_string()))
    );
    std::fs::remove_dir_all(dir).unwrap();
}
