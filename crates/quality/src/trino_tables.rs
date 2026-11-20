//! A `tables` job on Trino (decision 0048, point 2): the source's catalog
//! asked for its columns (`information_schema.columns`), the system's
//! schemas left out, and the answer made 0042's listing. Pure: the worker
//! runs the statement and hands the rows here.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::source_tables::{system_schemas, SourceColumn, SourceTable, TableListing, MAX_TABLES};
use crate::trino_catalog::catalog_name;

/// Every column of the source's tables, in order, but the system's.
pub fn columns_query(source: &str, driver: &str) -> String {
    let left_out: Vec<String> = system_schemas(driver)
        .iter()
        .map(|s| format!("'{s}'"))
        .collect();
    let filter = if left_out.is_empty() {
        String::new()
    } else {
        format!(" WHERE table_schema NOT IN ({})", left_out.join(", "))
    };
    format!(
        "SELECT table_schema, table_name, column_name, data_type FROM {}.information_schema.columns{filter} \
         ORDER BY table_schema, table_name, ordinal_position",
        catalog_name(source)
    )
}

/// The rows of [`columns_query`] as the source's listing: at most
/// [`MAX_TABLES`], said if cut. A row that is not four texts is skipped.
pub fn listing(source: &str, rows: &[Vec<Value>]) -> TableListing {
    let mut tables: BTreeMap<(String, String), Vec<SourceColumn>> = BTreeMap::new();
    for row in rows {
        let text = |i: usize| row.get(i).and_then(Value::as_str).map(str::to_string);
        let (Some(schema), Some(table), Some(name), Some(data_type)) =
            (text(0), text(1), text(2), text(3))
        else {
            continue;
        };
        tables
            .entry((schema, table))
            .or_default()
            .push(SourceColumn { name, data_type });
    }
    let truncated = tables.len() > MAX_TABLES;
    let tables = tables
        .into_iter()
        .take(MAX_TABLES)
        .map(|((schema, name), columns)| SourceTable {
            schema,
            name,
            columns,
        })
        .collect();
    TableListing {
        source: source.to_string(),
        tables,
        truncated,
    }
}
