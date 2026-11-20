//! A map job's reading of a source on Trino (decisions 0048 and 0053): the
//! statements for the table's columns (name, type, nullability) and for its
//! sample's aggregates, as `quality-rules-duckdb`'s profile reads them
//! through DuckDB, and the profile read back from their answers. Pure. Trino
//! gives no primary key, so none is proposed; patterns are anchored, as
//! `regexp_like` matches anywhere. Distinct values are `approx_distinct`: a
//! `count(DISTINCT …)` per column makes Trino plan a stage for each, and a
//! wide table's answer is then more than its client reads; the proposal
//! only compares them with thresholds. `tools/verify/trino_sqlserver.sh`
//! shows what Trino gives.

use serde_json::Value;

use crate::proposal::{ColumnProfile, Looks, Patterns};
use crate::trino_catalog::catalog_name;

/// Aggregates per column: empty, distinct, min and max length, then one per
/// pattern.
const PER_COLUMN: usize = 4 + 6;

/// Why a table cannot be profiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    /// Not `schema.table` (a three-part name keeps its last two).
    Table,
    /// The source has no such table, or Trino answered something else.
    Answer,
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The table's schema and name, as Trino names them: lowercase.
fn schema_and_table(table: &str) -> Result<(String, String), ProfileError> {
    let parts: Vec<&str> = table.split('.').collect();
    match parts.as_slice() {
        [.., schema, name] if !schema.is_empty() && !name.is_empty() => {
            Ok((schema.to_lowercase(), name.to_lowercase()))
        }
        _ => Err(ProfileError::Table),
    }
}

/// The statement for the table's columns: name, type and `YES` or `NO`
/// for nullable, in order.
pub fn columns_statement(source: &str, table: &str) -> Result<String, ProfileError> {
    let (schema, name) = schema_and_table(table)?;
    Ok(format!(
        "SELECT column_name, data_type, is_nullable FROM {}.information_schema.columns \
         WHERE table_schema = {} AND table_name = {} ORDER BY ordinal_position",
        catalog_name(source),
        literal(&schema),
        literal(&name),
    ))
}

/// The columns from the answer to [`columns_statement`].
pub fn read_columns(rows: &[Vec<Value>]) -> Result<Vec<(String, String, bool)>, ProfileError> {
    let columns = rows
        .iter()
        .map(|r| match r.as_slice() {
            [Value::String(n), Value::String(t), Value::String(null)] => {
                Ok((n.clone(), t.clone(), null == "YES"))
            }
            _ => Err(ProfileError::Answer),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if columns.is_empty() {
        return Err(ProfileError::Answer);
    }
    Ok(columns)
}

/// The statement for the sample's aggregates over the first `sample` rows:
/// the rows sampled, then [`PER_COLUMN`] counts per column.
pub fn profile_statement(
    source: &str,
    table: &str,
    columns: &[(String, String, bool)],
    sample: u64,
    patterns: &Patterns,
) -> Result<String, ProfileError> {
    let (schema, name) = schema_and_table(table)?;
    let mut parts = Vec::with_capacity(columns.len() * PER_COLUMN);
    for (column, _, _) in columns {
        let c = quoted(column);
        let v = format!("CAST({c} AS varchar)");
        // Empty as decision 0033 says: NULL, or blank (U+00A0 too).
        let empty = format!("({v} IS NULL OR regexp_like({v}, '^[\\s\\x{{00A0}}]*$'))");
        parts.push(format!("count(*) FILTER (WHERE {empty})"));
        parts.push(format!(
            "approx_distinct(CASE WHEN NOT {empty} THEN {c} END)"
        ));
        parts.push(format!("min(length({v})) FILTER (WHERE NOT {empty})"));
        parts.push(format!("max(length({v})) FILTER (WHERE NOT {empty})"));
        for (_, pattern) in patterns.all() {
            parts.push(format!(
                "count(*) FILTER (WHERE regexp_like({v}, {}))",
                literal(&format!("^(?:{pattern})$"))
            ));
        }
    }
    Ok(format!(
        "WITH s AS (SELECT * FROM {}.{}.{} LIMIT {sample}) SELECT count(*), {} FROM s",
        catalog_name(source),
        quoted(&schema),
        quoted(&name),
        parts.join(", ")
    ))
}

/// The rows sampled and each column's profile, from the answer to
/// [`profile_statement`].
pub fn read_profile(
    columns: Vec<(String, String, bool)>,
    rows: &[Vec<Value>],
) -> Result<(u64, Vec<ColumnProfile>), ProfileError> {
    let row = rows.first().ok_or(ProfileError::Answer)?;
    if row.len() != 1 + columns.len() * PER_COLUMN {
        return Err(ProfileError::Answer);
    }
    let n = |i: usize| row[i].as_u64().unwrap_or(0);
    let length = |i: usize| row[i].as_u64().and_then(|l| u32::try_from(l).ok());
    let profiles = columns
        .into_iter()
        .enumerate()
        .map(|(i, (name, declared_type, nullable))| {
            let at = 1 + i * PER_COLUMN;
            ColumnProfile {
                name,
                declared_type,
                nullable,
                primary_key: false,
                empty: n(at),
                distinct: n(at + 1),
                min_length: length(at + 2),
                max_length: length(at + 3),
                looks: Looks {
                    email: n(at + 4),
                    mobile: n(at + 5),
                    document: n(at + 6),
                    date: n(at + 7),
                    number: n(at + 8),
                    flag: n(at + 9),
                },
            }
        })
        .collect();
    Ok((n(0), profiles))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_tables_columns_are_asked_of_its_sources_catalog_lowercase() {
        assert_eq!(
            columns_statement("sqlserver_dev", "dbo.Clientes").unwrap(),
            "SELECT column_name, data_type, is_nullable FROM src_sqlserver_dev.information_schema.columns \
             WHERE table_schema = 'dbo' AND table_name = 'clientes' ORDER BY ordinal_position"
        );
        assert!(columns_statement("s", "db.dbo.t")
            .unwrap()
            .contains("table_schema = 'dbo'"));
        assert_eq!(columns_statement("s", "t"), Err(ProfileError::Table));
        let rows = vec![vec![json!("id"), json!("integer"), json!("NO")]];
        assert_eq!(
            read_columns(&rows).unwrap(),
            vec![("id".to_string(), "integer".to_string(), false)]
        );
        assert_eq!(read_columns(&[]), Err(ProfileError::Answer));
    }

    #[test]
    fn a_sample_is_its_first_rows_and_its_patterns_are_anchored() {
        let columns = vec![("email".to_string(), "varchar(60)".to_string(), true)];
        let sql = profile_statement("core", "dbo.t", &columns, 100, &Patterns::default()).unwrap();
        assert!(sql.starts_with("WITH s AS (SELECT * FROM src_core.\"dbo\".\"t\" LIMIT 100)"));
        assert!(sql.contains(r#"regexp_like(CAST("email" AS varchar), '^[\s\x{00A0}]*$')"#));
        assert!(sql.contains(r"'^(?:3\d{9})$'"));
        // All but the distinct values counted over a filter; those
        // approximated, so no stage is planned for each column.
        assert_eq!(sql.matches("FILTER").count(), PER_COLUMN - 1);
        assert!(sql.contains("approx_distinct(") && !sql.contains("DISTINCT CASE"));
    }

    #[test]
    fn a_profile_is_read_back_as_counts() {
        let columns = vec![("doc".to_string(), "varchar(12)".to_string(), false)];
        // As Trino answered the synthetic table of the verification.
        let row = vec![json!(5), json!(0), json!(4), json!(5), json!(8)]
            .into_iter()
            .chain([0, 0, 5, 0, 5, 0].map(|n| json!(n)))
            .collect();
        let (sampled, profiles) = read_profile(columns.clone(), &[row]).unwrap();
        assert_eq!(sampled, 5);
        let doc = &profiles[0];
        assert_eq!(
            (doc.empty, doc.distinct, doc.min_length, doc.max_length),
            (0, 4, Some(5), Some(8))
        );
        assert_eq!(
            (doc.looks.document, doc.primary_key, doc.nullable),
            (5, false, false)
        );
        assert_eq!(
            read_profile(columns, &[vec![json!(1)]]),
            Err(ProfileError::Answer)
        );
    }
}
