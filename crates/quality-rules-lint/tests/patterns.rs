//! _Split by field_'s second pass (decisions 0044 and 0045): a rule over
//! several fields, or with `for_each`, is written per field with the id it
//! ran with; a pattern fields share becomes a named rule they extend, each
//! keeping its alert; the same rules run. Synthetic rules.

use quality_rules_compile::{FieldType, Schema};
use quality_rules_lint::split_and_extract;

fn schema() -> Schema {
    let mut s = Schema::default();
    for name in ["Name", "Surname", "Phone1", "Phone2", "Kind"] {
        s.fields.insert(name.into(), FieldType::Text);
    }
    s
}

const GROUP: &str = "\
dataset: clients
rules:
  # Names are always written.
  - id: NA-01
    field: [Name, Surname]
    check: required
    severity: major
    cause: capture
    alert: No name

  - id: PH-01
    field: Phone{n}
    for_each: { n: ['1', '2'] }
    check: format
    pattern: '[0-9]{7,10}'
    severity: minor
    cause: capture
    alert: Phone {n} is not a number

  - id: KI-01
    field: Kind
    check: required
    severity: major
    cause: capture
    alert: No kind
";

fn text(changes: &[(String, Option<String>)], file: &str) -> String {
    changes
        .iter()
        .find(|(f, _)| f == file)
        .and_then(|(_, t)| t.clone())
        .unwrap_or_else(|| panic!("{file} written"))
}

#[test]
fn rules_over_several_fields_go_to_each_and_shared_patterns_are_named() {
    let files = vec![("group.yaml".to_string(), GROUP.to_string())];
    let changes = split_and_extract(&files, &schema()).unwrap();
    let names: Vec<&str> = changes.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(
        names,
        [
            "reglas_globales.yaml",
            "Name.yaml",
            "Surname.yaml",
            "Phone1.yaml",
            "Phone2.yaml",
            "Kind.yaml",
            "group.yaml"
        ]
    );
    assert_eq!(
        text(&changes, "reglas_globales.yaml"),
        "dataset: clients\n\nnamed:\n  \"NA-01\":\n    check: required\n    severity: major\n    cause: capture\n  \"PH-01\":\n    check: format\n    pattern: '[0-9]{7,10}'\n    severity: minor\n    cause: capture\n"
    );
    // The first field keeps the comment; each its id as it ran.
    assert_eq!(
        text(&changes, "Name.yaml"),
        "dataset: clients\nrules:\n  # Names are always written.\n  - id: \"NA-01.Name\"\n    field: \"Name\"\n    extends: \"NA-01\"\n    alert: No name\n"
    );
    assert_eq!(
        text(&changes, "Kind.yaml"),
        "dataset: clients\nrules:\n  - id: \"KI-01\"\n    field: \"Kind\"\n    extends: \"NA-01\"\n    alert: No kind\n"
    );
    // `for_each` written out per value, each with its alert.
    assert_eq!(
        text(&changes, "Phone2.yaml"),
        "dataset: clients\nrules:\n  - id: \"PH-01.2\"\n    field: \"Phone2\"\n    extends: \"PH-01\"\n    alert: Phone 2 is not a number\n"
    );
    assert_eq!(changes.last().unwrap().1, None, "the group's file is gone");
}

#[test]
fn files_done_already_change_nothing() {
    let files = vec![("group.yaml".to_string(), GROUP.to_string())];
    let done: Vec<(String, String)> = split_and_extract(&files, &schema())
        .unwrap()
        .into_iter()
        .filter_map(|(f, t)| t.map(|t| (f, t)))
        .collect();
    assert_eq!(split_and_extract(&done, &schema()).unwrap(), Vec::new());
}

#[test]
fn a_named_rule_of_the_same_name_is_not_written_over() {
    let globals = "dataset: clients\nnamed:\n  NA-01:\n    check: required\n    severity: minor\n    cause: capture\n# after\n";
    let files = vec![
        ("reglas_globales.yaml".to_string(), globals.to_string()),
        ("group.yaml".to_string(), GROUP.to_string()),
    ];
    let changes = split_and_extract(&files, &schema()).unwrap();
    let global = text(&changes, "reglas_globales.yaml");
    assert!(global.contains("  NA-01:\n    check: required\n    severity: minor\n    cause: capture\n  \"NA-01-2\":\n    check: required\n    severity: major\n"), "{global}");
    assert!(text(&changes, "Kind.yaml").contains("extends: \"NA-01-2\""));
}
