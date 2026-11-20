//! Fields named as their columns (decision 0044): a dataset's rules renamed
//! where a name is a field (expressions, lists, examples; another
//! dataset's in references and `'alias.Field'`), never in what people
//! read, a regex, a list of values or a text in quotes; refused unless the
//! same rules read the same fields by their new names. Synthetic rules.

use std::collections::BTreeMap;

use quality_rules_compile::{FieldType, Schema};
use quality_rules_lint::{rename_fields, Renames};

fn schema(names: &[&str]) -> Schema {
    let mut s = Schema::default();
    for name in names {
        s.fields.insert((*name).into(), FieldType::Text);
    }
    s
}

fn renames(pairs: &[(&str, &str)]) -> Renames {
    pairs
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

const RULES: &str = "\
dataset: clients
references:
  advisors: { dataset: advisors, key: Code, where: \"State = 'ACTIVE'\" }
rules:
  # Kind is checked against Code.
  - id: R-01
    field: Kind
    check: required
    when: Code IS NOT NULL AND Kind <> 'Code'
    severity: major
    cause: capture
    alert: Kind is missing while Code is there
    examples:
      fails: { Kind: null, Code: '7' }

  - id: R-02
    field: [Code, Kind]
    check: format
    pattern: 'Code[0-9]+'
    severity: minor
    cause: capture
    alert: Bad
  - { id: R-03, check: expression, dimension: consistency, fails: \"NOT in_reference(Code, 'advisors.Code')\", severity: minor, cause: capture, alert: Code }
";

#[test]
fn a_field_is_renamed_where_it_is_one_and_nowhere_else() {
    let files = vec![("clients.yaml".to_string(), RULES.to_string())];
    let own = renames(&[("Code", "CODIGO"), ("Kind", "TIPO")]);
    let others = BTreeMap::from([(
        "advisors".to_string(),
        renames(&[("Code", "CodEscala"), ("State", "Estado")]),
    )]);
    let out = rename_fields(
        &files,
        (&schema(&["Code", "Kind"]), &schema(&["CODIGO", "TIPO"])),
        &own,
        &others,
    )
    .expect("renamed");
    assert_eq!(
        out[0].1,
        "\
dataset: clients
references:
  advisors: { dataset: advisors, key: CodEscala, where: \"Estado = 'ACTIVE'\" }
rules:
  # Kind is checked against Code.
  - id: R-01
    field: TIPO
    check: required
    when: CODIGO IS NOT NULL AND TIPO <> 'Code'
    severity: major
    cause: capture
    alert: Kind is missing while Code is there
    examples:
      fails: { TIPO: null, CODIGO: '7' }

  - id: R-02
    field: [CODIGO, TIPO]
    check: format
    pattern: 'Code[0-9]+'
    severity: minor
    cause: capture
    alert: Bad
  - { id: R-03, check: expression, dimension: consistency, fails: \"NOT in_reference(CODIGO, 'advisors.CodEscala')\", severity: minor, cause: capture, alert: Code }
"
    );
}

#[test]
fn a_rename_that_would_change_what_runs_is_refused() {
    let files = vec![("clients.yaml".to_string(), RULES.to_string())];
    // The schema after does not have the new name: the rules would not read.
    let own = renames(&[("Code", "CODIGO")]);
    let refused = rename_fields(
        &files,
        (&schema(&["Code", "Kind"]), &schema(&["Code", "Kind"])),
        &own,
        &BTreeMap::new(),
    );
    assert_eq!(refused, None);
}
