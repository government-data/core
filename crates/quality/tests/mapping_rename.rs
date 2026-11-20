//! Fields named as their columns (decision 0044): which are named
//! otherwise, refused when two would share a name, and the mapping's text
//! with them renamed, the rest as written. Synthetic names only.

use quality::mapping::Mapping;
use quality::mapping_rename::{renamed_mapping, renames, Clash};

const MAPPING: &str = "\
dataset: clients
source: clients
key:
  - Id
fields:
  # The client's code.
  Id: { column: CLIENT_ID, type: text }
  \"Kind\":
    column: Kind
    type: text
  Opened:
    column: opened on
    type: date
";

#[test]
fn fields_take_their_columns_names_and_the_rest_stays() {
    let mapping = Mapping::parse(MAPPING).unwrap();
    let r = renames(&mapping).unwrap();
    let pairs: Vec<(&str, &str)> = r.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    assert_eq!(pairs, [("Id", "CLIENT_ID"), ("Opened", "OpenedOn")]);
    let text = renamed_mapping(MAPPING, &r);
    assert_eq!(
        text,
        MAPPING
            .replace("  - Id\n", "  - CLIENT_ID\n")
            .replace("  Id: {", "  CLIENT_ID: {")
            .replace("  Opened:\n", "  OpenedOn:\n")
    );
    let after = Mapping::parse(&text).unwrap();
    assert_eq!(after.key, ["CLIENT_ID"]);
    assert_eq!(after.fields["OpenedOn"].value.column, "opened on");
}

#[test]
fn two_fields_that_would_share_a_name_are_refused() {
    // `a b` would be named `AB`, the name of a field that stays.
    let yaml = "dataset: c\nsource: c\nfields:\n  A: { column: a b, type: text }\n  AB: { column: AB, type: text }\n";
    let mapping = Mapping::parse(yaml).unwrap();
    assert_eq!(renames(&mapping), Err(Clash { name: "AB".into() }));
    // Names swapped all at once are not a clash.
    let swap = "dataset: c\nsource: c\nfields:\n  A: { column: Code, type: text }\n  Code: { column: Other, type: text }\n";
    assert!(renames(&Mapping::parse(swap).unwrap()).is_ok());
}
