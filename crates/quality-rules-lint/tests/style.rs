//! How a rules file is written (decision 0041): each style warning on a
//! synthetic file that has it, at its place, and not on one that does not.

use quality_rules_compile::{FieldType, Schema};
use quality_rules_lint::{lint, Code, Severity};

fn schema() -> Schema {
    let mut s = Schema::default();
    for name in ["Name", "Code"] {
        s.fields.insert(name.into(), FieldType::Text);
    }
    s
}

/// A tidy file: no style warning.
const TIDY: &str = "\
dataset: d
lists:
  kinds: [A, B, '9609', 'yes']
rules:
  - id: R-01
    field: Name
    check: required
    severity: major
    cause: capture
    alert: Campo vacío

  # The second rule.
  - id: R-02
    field: Code
    check: expression
    fails: Code IN ('0', '00') OR coalesce(Name, '') LIKE 'X%'
    dimension: validity
    severity: minor
    cause: capture
    alert: ':cut is kept quoted'
";

/// The style codes found in `yaml`, with their line and column.
fn style(yaml: &str) -> Vec<(Code, u64, u64)> {
    lint(yaml, &schema())
        .findings
        .into_iter()
        .filter(|f| f.code.id().starts_with("QR2") && f.code.id() >= "QR200")
        .map(|f| {
            assert_eq!(f.severity(), Severity::Warning, "{f:?}");
            let at = f.at.expect("a place");
            (f.code, at.line, at.column)
        })
        .collect()
}

#[test]
fn a_tidy_file_has_no_style_warning() {
    assert_eq!(style(TIDY), []);
}

#[test]
fn quotes_a_value_does_not_need_are_found() {
    let yaml = TIDY
        .replace("check: required", "check: \"required\"")
        .replace("alert: Campo vacío", "alert: 'Campo vacío'")
        .replace("[A, B,", "[\"A\", B,");
    assert_eq!(
        style(&yaml),
        [
            (Code::NeedlessQuotes, 3, 11),
            (Code::NeedlessQuotes, 7, 12),
            (Code::NeedlessQuotes, 10, 12)
        ]
    );
}

#[test]
fn quotes_a_value_needs_are_kept() {
    for value in [
        "'9609'",
        "'yes'",
        "'No'",
        "'null'",
        "'2024-01-31'",
        "':cut'",
        "'a: b'",
        "'a #b'",
        "'- x'",
        "' x'",
        "''",
        "\"a\\tb\"",
    ] {
        let yaml = TIDY.replace("alert: ':cut is kept quoted'", &format!("alert: {value}"));
        assert_eq!(style(&yaml), [], "{value}");
    }
}

#[test]
fn whitespace_is_found_where_it_is() {
    let yaml = TIDY
        .replace("severity: major", "severity: major  ")
        .replace(
            "cause: capture\n    alert: Campo",
            "cause:\tcapture\n    alert: Campo",
        );
    assert_eq!(
        style(&yaml),
        [(Code::TrailingSpace, 8, 20), (Code::Tab, 9, 11)]
    );
}

#[test]
fn indentation_is_two_spaces_a_level() {
    let yaml = TIDY.replace("    field: Name\n", "      field: Name\n");
    assert_eq!(style(&yaml), [(Code::Indentation, 6, 1)]);
    let odd = TIDY.replace("  - id: R-01", "   - id: R-01");
    assert!(style(&odd).contains(&(Code::Indentation, 5, 1)));
}

#[test]
fn a_long_line_is_found_past_its_limit() {
    let long = format!("    note: {}\n", "x".repeat(120));
    let yaml = TIDY.replace(
        "    cause: capture\n    alert: Campo",
        &format!("{long}    cause: capture\n    alert: Campo"),
    );
    let found = lint(&yaml, &schema()).findings;
    let f = found
        .iter()
        .find(|f| f.code == Code::LongLine)
        .expect("QR204");
    assert_eq!((f.at.unwrap().line, f.at.unwrap().column), (9, 121));
    assert!(f.values.contains(&("length", "130".to_string())));
}

#[test]
fn each_rule_but_the_first_follows_one_blank_line() {
    let none = TIDY.replace("\n\n  # The second rule.", "\n  # The second rule.");
    assert_eq!(style(&none), [(Code::RuleSpacing, 12, 3)]);
    let two = TIDY.replace("\n\n  # The second", "\n\n\n  # The second");
    assert_eq!(style(&two), [(Code::RuleSpacing, 14, 3)]);
}

#[test]
fn style_is_checked_even_when_the_yaml_does_not_read() {
    let codes: Vec<Code> = lint("rules: [\n  x:\t1  \n", &schema())
        .findings
        .into_iter()
        .map(|f| f.code)
        .collect();
    assert!(codes.contains(&Code::Syntax));
    assert!(codes.contains(&Code::Tab) && codes.contains(&Code::TrailingSpace));
}

#[test]
fn a_value_lost_to_a_comment_is_found() {
    let block = TIDY.replace("alert: Campo vacío", "alert: #1651");
    let found = lint(&block, &schema()).findings;
    let f = found
        .iter()
        .find(|f| f.code == Code::LostValue)
        .expect("QR206");
    assert_eq!((f.at.unwrap().line, f.at.unwrap().column), (10, 12));
    assert!(f.values.contains(&("value", "#1651".to_string())));
    let comment = TIDY.replace(
        "cause: capture\n    alert: Campo",
        "cause: capture # why\n    alert: Campo",
    );
    assert_eq!(style(&comment), [], "a comment is not a lost value");
}

#[test]
fn a_yaml_error_caused_by_a_lost_value_is_said_there() {
    let yaml = TIDY.replace("[A, B,", "[A, #B,");
    let found = lint(&yaml, &schema()).findings;
    let syntax = found
        .iter()
        .find(|f| f.code == Code::Syntax)
        .expect("QR001");
    assert_eq!(
        (syntax.at.unwrap().line, syntax.at.unwrap().column),
        (3, 14)
    );
    assert_eq!(
        syntax.values,
        [("message_key", "quality.lint.syntax.lost_value".to_string())]
    );
    assert!(found.iter().all(|f| f.code != Code::LostValue), "said once");
}

#[test]
fn a_quote_left_open_is_found_and_said_as_the_cause() {
    let wrapped = TIDY.replace("alert: Campo vacío", "alert: \"Campo\n      vacío\"");
    assert_eq!(style(&wrapped), [(Code::UnclosedQuote, 10, 12)]);
    let open = TIDY.replace("severity: major", "severity: \"major } # here");
    let found = lint(&open, &schema()).findings;
    let syntax = found
        .iter()
        .find(|f| f.code == Code::Syntax)
        .expect("QR001");
    assert_eq!(
        (syntax.at.unwrap().line, syntax.at.unwrap().column),
        (8, 15)
    );
    assert_eq!(
        syntax.values,
        [(
            "message_key",
            "quality.lint.syntax.unclosed_quote".to_string()
        )]
    );
    assert!(
        found.iter().all(|f| f.code != Code::UnclosedQuote),
        "said once"
    );
}

#[test]
fn a_value_continued_at_its_keys_indent_is_said_where_it_goes_on() {
    let continued = TIDY.replace(
        "lists:\n  kinds: [A, B, '9609', 'yes']\n",
        "derived:\n  Total: CASE WHEN Code = 'A'\n  THEN 1 \n  END\nlists:\n  kinds: [A, B, '9609', 'yes']\n",
    );
    let found = lint(&continued, &schema()).findings;
    let syntax = found
        .iter()
        .find(|f| f.code == Code::Syntax)
        .expect("QR001");
    assert_eq!((syntax.at.unwrap().line, syntax.at.unwrap().column), (4, 3));
    assert_eq!(
        syntax.values,
        [("message_key", "quality.lint.syntax.continued".to_string())]
    );
    assert!(found.iter().all(|f| f.code != Code::Continued), "said once");
    let deeper = continued.replace("\n  THEN 1 \n  END\n", "\n    THEN 1\n    END\n");
    assert!(lint(&deeper, &schema())
        .findings
        .iter()
        .all(|f| f.code != Code::Continued && f.code != Code::Syntax));
}

#[test]
fn a_map_made_one_text_is_said_at_its_block_header() {
    let block = TIDY.replace(
        "lists:
  kinds: [A, B, '9609', 'yes']
",
        "lists: >-
  kinds: [A, B]
",
    );
    let found = lint(&block, &schema()).findings;
    let syntax = found
        .iter()
        .find(|f| f.code == Code::Syntax)
        .expect("QR001");
    assert_eq!((syntax.at.unwrap().line, syntax.at.unwrap().column), (2, 8));
    assert_eq!(
        syntax.values,
        [(
            "message_key",
            "quality.lint.syntax.block_on_map".to_string()
        )]
    );
    let note = TIDY.replace(
        "  - id: R-01
",
        "  - id: R-01
    note: >-
      a text
",
    );
    assert!(lint(&note, &schema())
        .findings
        .iter()
        .all(|f| f.code != Code::BlockOnMap));
}

#[test]
fn a_rule_in_another_fields_file_is_found_where_it_starts() {
    let placed = |file: &str| -> Vec<(Code, u64, String)> {
        let report = lint(TIDY, &schema());
        quality_rules_lint::placement(file, TIDY, &report)
            .into_iter()
            .map(|f| {
                assert_eq!(f.severity(), Severity::Warning);
                let file = f.values.iter().find(|(k, _)| *k == "file").unwrap();
                (f.code, f.at.unwrap().line, file.1.clone())
            })
            .collect()
    };
    // R-01 is Name's; R-02 is Code's by its field.
    assert_eq!(
        placed("Name.yaml"),
        [(Code::OtherFieldsFile, 13, "Code.yaml".into())]
    );
    assert_eq!(placed("Code.yaml").len(), 1);
    assert_eq!(placed("identification.yaml").len(), 2);
    // An expression with no field is its first field read's.
    let expr = "dataset: d\nrules:\n  - id: X\n    check: expression\n    fails: Code = '1' AND Name = 'A'\n    dimension: validity\n    severity: minor\n    cause: capture\n    alert: x\n";
    let report = lint(expr, &schema());
    let on_name = quality_rules_lint::placement("Name.yaml", expr, &report);
    let on_code = quality_rules_lint::placement("Code.yaml", expr, &report);
    assert_eq!((on_name.len(), on_code.len()), (1, 0), "{on_name:?}");
}
