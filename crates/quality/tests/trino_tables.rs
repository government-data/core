//! A `tables` job on Trino (decision 0048, point 2), pure: the statement
//! asks the source's catalog for its columns, the system's schemas left
//! out, and Trino's rows become the listing, tables in order with their
//! columns in theirs. Synthetic names only.

use quality::source_tables::MAX_TABLES;
use quality::trino_tables::{columns_query, listing};
use serde_json::{json, Value};

#[test]
fn the_catalogs_columns_are_asked_without_the_systems_schemas() {
    assert_eq!(
        columns_query("ledger", "sqlserver"),
        "SELECT table_schema, table_name, column_name, data_type \
         FROM src_ledger.information_schema.columns \
         WHERE table_schema NOT IN ('information_schema', 'sys') \
         ORDER BY table_schema, table_name, ordinal_position"
    );
}

fn row(schema: &str, table: &str, column: &str, data_type: &str) -> Vec<Value> {
    vec![json!(schema), json!(table), json!(column), json!(data_type)]
}

#[test]
fn trinos_rows_are_the_tables_with_their_columns_in_order() {
    let rows = [
        row("dbo", "clients", "id", "integer"),
        row("dbo", "clients", "code", "varchar(10)"),
        vec![json!("dbo"), Value::Null, json!("x"), json!("integer")],
        row("loans", "credits", "amount", "decimal(12,2)"),
    ];
    let listed = listing("ledger", &rows);
    assert_eq!(listed.source, "ledger");
    assert!(!listed.truncated);
    let named: Vec<_> = listed
        .tables
        .iter()
        .map(|t| {
            let columns: Vec<_> = t
                .columns
                .iter()
                .map(|c| format!("{} {}", c.name, c.data_type))
                .collect();
            format!("{}.{}: {}", t.schema, t.name, columns.join(", "))
        })
        .collect();
    assert_eq!(
        named,
        [
            "dbo.clients: id integer, code varchar(10)",
            "loans.credits: amount decimal(12,2)"
        ]
    );
}

#[test]
fn more_tables_than_a_listing_keeps_are_cut_and_said() {
    let rows: Vec<_> = (0..=MAX_TABLES)
        .map(|i| row("dbo", &format!("t{i:05}"), "id", "integer"))
        .collect();
    let listed = listing("ledger", &rows);
    assert_eq!(listed.tables.len(), MAX_TABLES);
    assert!(listed.truncated);
}
