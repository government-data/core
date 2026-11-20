//! _Add to the mapping_ (decision 0043) as text: the column becomes a field
//! at the end of `fields:`, typed from its source type, the rest of the
//! file as written. Synthetic names only.

use quality::mapping::Mapping;
use quality::mapping_field::{kind_of, started, with_columns, with_field, NotAdded, Start};

const MAPPING: &str = "dataset: loans\nsource: loans\nkey: [Id]\nfields:\n  # what identifies a loan\n  Id: { column: id, type: text }\n\n    \n# after\ndescription: Loans\n";

#[test]
fn a_column_becomes_a_field_at_the_end_of_fields() {
    let (text, field) = with_field(MAPPING, "loan_amount", "DECIMAL(12,2)").unwrap();
    assert_eq!(field, "loan_amount");
    assert_eq!(
        text,
        "dataset: loans\nsource: loans\nkey: [Id]\nfields:\n  # what identifies a loan\n  Id: { column: id, type: text }\n  loan_amount: { column: \"loan_amount\", type: number }\n\n    \n# after\ndescription: Loans\n"
    );
    let mapping = Mapping::parse(&text).unwrap();
    assert_eq!(mapping.fields["loan_amount"].value.column, "loan_amount");
    // And another after it.
    let (text, _) = with_field(&text, "Opened", "DATE").unwrap();
    assert!(text.contains("number }\n  Opened: { column: \"Opened\", type: date }\n"));
}

#[test]
fn a_column_read_or_a_name_taken_is_not_added() {
    assert_eq!(with_field(MAPPING, "id", "VARCHAR"), Err(NotAdded::Read));
    assert_eq!(with_field(MAPPING, "-id", "VARCHAR"), Err(NotAdded::Taken));
    assert_eq!(
        with_field("dataset: [", "x", "VARCHAR"),
        Err(NotAdded::Mapping)
    );
    let flow = "dataset: loans\nsource: loans\nfields: { Id: { column: id, type: text } }\n";
    assert_eq!(with_field(flow, "x", "VARCHAR"), Err(NotAdded::Mapping));
}

#[test]
fn a_fields_type_follows_its_columns() {
    let kinds = [
        ("VARCHAR", "text"),
        ("integer", "number"),
        ("BIGINT", "number"),
        ("DOUBLE", "number"),
        ("DATE", "date"),
        ("TIMESTAMP WITH TIME ZONE", "date"),
        ("BOOLEAN", "boolean"),
        ("UUID", "text"),
    ];
    for (declared, kind) in kinds {
        assert_eq!(kind_of(declared), kind, "{declared}");
    }
}

#[test]
fn a_lake_table_starts_a_dataset_that_reads_it() {
    let start = Start {
        source: None,
        table: "advisors",
    };
    let (text, field) = started("advisors", start, "Zone", "INTEGER").unwrap();
    assert_eq!(field, "Zone");
    assert_eq!(
        text,
        "dataset: advisors\nsource: \"advisors\"\nfields:\n  Zone: { column: \"Zone\", type: number }\n"
    );
    let bad = Start {
        source: Some("core"),
        table: "a.b.c.d",
    };
    assert_eq!(
        started("advisors", bad, "Zone", "INTEGER"),
        Err(NotAdded::Mapping)
    );
}

#[test]
fn a_tables_columns_are_added_but_those_read() {
    let columns = [
        ("id", "VARCHAR"),
        ("Opened", "DATE"),
        ("loan_amount", "INTEGER"),
    ];
    let (text, fields) = with_columns(MAPPING.to_string(), &columns).unwrap();
    assert_eq!(fields, ["Opened", "loan_amount"]);
    assert!(text.contains("  Opened: { column: \"Opened\", type: date }\n  loan_amount: { column: \"loan_amount\", type: number }\n"));
    let taken = [("x", "TEXT"), ("-id", "TEXT")];
    let refused = with_columns(MAPPING.to_string(), &taken);
    assert_eq!(refused, Err((NotAdded::Taken, "-id".to_string())));
}
