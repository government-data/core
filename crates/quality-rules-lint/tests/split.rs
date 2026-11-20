//! _Split by field_ (decisions 0044 and 0045): files by field group become
//! the table's global rules and a file per field, comments with their
//! rules, and the same rules run. Synthetic rules.

use quality_rules_compile::{FieldType, Schema};
use quality_rules_lint::{split_by_field, SplitRefused};

fn schema() -> Schema {
    let mut s = Schema::default();
    for name in ["Document", "Email", "Kind", "Ciiu"] {
        s.fields.insert(name.into(), FieldType::Text);
    }
    s
}

const IDENTIFICATION: &str = "\
# Identification of a client.
dataset: clients
lists:
  kinds: [PN, PJ]
rules:
  # The document is always there.
  - id: ID-01
    field: Document
    check: required
    severity: major
    cause: capture
    alert: No document

  - id: ID-02
    field: Kind
    check: domain
    in: { list: kinds }
    severity: minor
    cause: capture
    alert: Unknown kind

  - id: ID-03
    check: expression
    fails: Kind = 'PJ' AND Document IS NULL
    dimension: completeness
    severity: major
    cause: capture
    alert: A company with no document
";

const CONTACT: &str = "\
dataset: clients
lists:
  kinds: [PN, PJ]
rules:
  - id: CO-01
    field: Email
    check: required
    severity: minor
    cause: capture
    alert: No email
";

fn files() -> Vec<(String, String)> {
    vec![
        ("identification.yaml".into(), IDENTIFICATION.into()),
        ("contact.yaml".into(), CONTACT.into()),
    ]
}

fn text<'a>(changes: &'a [(String, Option<String>)], file: &str) -> Option<&'a str> {
    changes
        .iter()
        .find(|(f, _)| f == file)
        .and_then(|(_, t)| t.as_deref())
}

#[test]
fn files_by_group_become_global_rules_and_a_file_a_field() {
    let changes = split_by_field(&files(), &schema()).unwrap();
    let global = text(&changes, "reglas_globales.yaml").unwrap();
    assert!(global.starts_with("dataset: clients\n"), "{global}");
    assert_eq!(
        global.matches("kinds:").count(),
        1,
        "declared once: {global}"
    );

    let document = text(&changes, "Document.yaml").unwrap();
    assert!(
        document.contains("# The document is always there.\n  - id: ID-01"),
        "{document}"
    );
    assert!(
        document.contains("id: ID-03"),
        "an expression goes with the first field it reads"
    );
    assert!(text(&changes, "Kind.yaml").unwrap().contains("id: ID-02"));
    assert!(text(&changes, "Email.yaml").unwrap().contains("id: CO-01"));

    // The group files are removed.
    for gone in ["identification.yaml", "contact.yaml"] {
        assert!(changes.contains(&(gone.to_string(), None)), "{gone}");
    }
}

#[test]
fn files_already_split_change_nothing() {
    let changes = split_by_field(&files(), &schema()).unwrap();
    let split: Vec<(String, String)> = changes
        .into_iter()
        .filter_map(|(f, t)| t.map(|t| (f, t)))
        .collect();
    assert_eq!(split_by_field(&split, &schema()).unwrap(), []);
}

#[test]
fn what_cannot_be_split_is_said() {
    let differs = CONTACT.replace("[PN, PJ]", "[PN]");
    let refused = split_by_field(
        &[
            ("identification.yaml".into(), IDENTIFICATION.into()),
            ("contact.yaml".into(), differs),
        ],
        &schema(),
    );
    assert_eq!(
        refused,
        Err(SplitRefused::Conflict {
            kind: "lists".into(),
            name: "kinds".into()
        })
    );
    let inline = CONTACT.replace("lists:\n  kinds: [PN, PJ]", "lists: { kinds: [PN, PJ] }");
    assert!(matches!(
        split_by_field(&[("contact.yaml".into(), inline)], &schema()),
        Err(SplitRefused::Inline { .. })
    ));
    let broken = CONTACT.replace("field: Email", "field: Nope");
    assert!(matches!(
        split_by_field(&[("contact.yaml".into(), broken)], &schema()),
        Err(SplitRefused::Errors { .. })
    ));
}
