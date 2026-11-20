//! A table's global rules and the rules that extend them (decision 0045).
//! Synthetic rules.

use quality_rules_model::{parse, with_globals, Pattern, Position, Severity, Unmerged};

const GLOBALS: &str = r#"dataset: clients
lists:
  ciiu: ["0111", "8430"]
derived:
  Equity: Assets - Liabilities
named:
  valid_document:
    check: format
    pattern: '\d{5,12}'
    severity: major
    cause: capture
    alert: Between 5 and 12 digits
"#;

const FIELD: &str = r#"dataset: clients
rules:
  - id: Document-01
    field: Document
    extends: valid_document

  - id: Document-02
    field: Representative
    extends: valid_document
    severity: minor
    when: Kind = 'LEGAL'

  - id: Ciiu-01
    field: Ciiu
    check: domain
    in: { list: ciiu }
"#;

#[test]
fn a_rule_takes_the_named_rule_and_changes_what_it_says() {
    let (file, problems) = with_globals(&parse(FIELD).unwrap(), &parse(GLOBALS).unwrap());
    assert_eq!(problems, []);
    let plain = &file.rules[0];
    assert_eq!(plain.check().value, Pattern::Format);
    assert_eq!(plain.pattern.as_ref().unwrap().value, r"\d{5,12}");
    assert_eq!(plain.severity.as_ref().unwrap().value, Severity::Major);
    // What it takes is placed at its `extends`, where lint will speak.
    let extends = plain.extends.as_ref().unwrap().at;
    assert_eq!(plain.check().at, extends);
    assert_eq!(
        extends,
        Position {
            line: 5,
            column: 14
        }
    );

    let changed = &file.rules[1];
    assert_eq!(changed.id.value, "Document-02");
    assert_eq!(
        changed.severity.as_ref().unwrap().value,
        Severity::Minor,
        "the field's wins"
    );
    assert_eq!(changed.when.as_ref().unwrap().value, "Kind = 'LEGAL'");
    assert_eq!(
        changed.alert.as_ref().unwrap().value,
        "Between 5 and 12 digits"
    );

    // The declarations are the file's to use.
    assert!(file.lists.contains_key("ciiu"));
    assert!(file.derived.contains_key("Equity"));
}

#[test]
fn what_does_not_merge_is_said_where_it_is() {
    let field = "dataset: clients\nlists:\n  ciiu: [\"1\"]\nrules:\n  - id: X-01\n    field: X\n    extends: nowhere\n";
    let globals = format!("{GLOBALS}rules:\n  - id: G-01\n    field: X\n    check: required\n");
    let (file, problems) = with_globals(&parse(field).unwrap(), &parse(&globals).unwrap());
    assert!(file.rules.is_empty(), "an unknown extends is left out");
    assert_eq!(
        problems,
        [
            Unmerged::RulesInGlobals {
                at: Position {
                    line: 14,
                    column: 9
                }
            },
            Unmerged::Twice {
                kind: "list",
                name: "ciiu".into(),
                at: Position { line: 3, column: 9 }
            },
            Unmerged::UnknownNamed {
                rule: "X-01".into(),
                name: "nowhere".into(),
                at: Position {
                    line: 7,
                    column: 14
                }
            },
        ]
    );
}

#[test]
fn parse_refuses_a_rule_with_no_pattern_and_a_named_rule_with_an_id() {
    let no_pattern = parse("dataset: d\nrules:\n  - id: R-1\n    field: A\n").unwrap_err();
    assert!(no_pattern.message.contains("extends"), "{no_pattern}");
    let no_id = parse("dataset: d\nrules:\n  - field: A\n    check: required\n").unwrap_err();
    assert!(no_id.message.contains("id"), "{no_id}");
    for named in [
        "named:\n  n:\n    id: N\n    check: required\n",
        "named:\n  n:\n    field: A\n    check: required\n",
        "named:\n  n:\n    extends: m\n",
        "named:\n  n:\n    severity: major\n",
    ] {
        let e = parse(&format!("dataset: d\n{named}")).unwrap_err();
        assert!(e.message.contains("named rule"), "{named}: {e}");
    }
    assert!(
        parse("dataset: d\n").is_ok(),
        "a file of global rules needs no rules"
    );
}
