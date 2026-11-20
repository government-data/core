//! A map job's reading of a source's table (decision 0038): its metadata as
//! DuckDB sees the attached source (columns, types, nullability, primary
//! key) and its sample's aggregates in one query (empties, distinct values,
//! lengths, and the values matching each pattern of `quality::proposal`).
//! Only counts come out; `tools/verify/source_map.py` shows what DuckDB
//! gives, and that the attached source still reads once locked down.

use quality::proposal::{ColumnProfile, Looks, Patterns};
use quality_rules_sql::quote_text;

use crate::{invalid, quoted, Error, Runner};

/// Aggregates per column: empty, distinct, min and max length, then one per
/// pattern.
const PER_COLUMN: usize = 4 + 6;

impl Runner {
    /// Whether DuckDB, the engine the patterns run on, takes `pattern`.
    pub fn takes_pattern(&self, pattern: &str) -> bool {
        let Ok(p) = quote_text(pattern) else {
            return false;
        };
        self.con
            .execute_batch(&format!("SELECT regexp_full_match('', {p})"))
            .is_ok()
    }

    /// Locks the connection down with nothing but what is attached: a map
    /// reads its source and writes nothing (decision 0033's lockdown).
    pub fn lock_down(&mut self) -> Result<(), Error> {
        self.con
            .execute_batch("SET enable_external_access = false; SET lock_configuration = true;")
    }

    /// The table's columns with their metadata, and its sample's
    /// aggregates over its first `sample` rows, matched against `patterns`:
    /// the rows sampled and each column's profile.
    pub fn profile_source(
        &self,
        table: &str,
        sample: u64,
        patterns: &Patterns,
    ) -> Result<(u64, Vec<ColumnProfile>), Error> {
        let (schema, name) = match table.split_once('.') {
            Some((s, t)) => (s, t),
            None => ("public", table),
        };
        let mut stmt = self.con.prepare(
            "SELECT column_name, data_type, is_nullable FROM duckdb_columns() \
             WHERE database_name = 'src' AND schema_name = ? AND table_name = ? \
             ORDER BY column_index",
        )?;
        let columns: Vec<(String, String, bool)> = stmt
            .query_map([schema, name], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?;
        if columns.is_empty() {
            return Err(invalid(format!("the source has no table {table}")));
        }
        let mut stmt = self.con.prepare(
            "SELECT unnest(constraint_column_names) FROM duckdb_constraints() \
             WHERE database_name = 'src' AND schema_name = ? AND table_name = ? \
             AND constraint_type = 'PRIMARY KEY'",
        )?;
        let key: Vec<String> = stmt
            .query_map([schema, name], |r| r.get(0))?
            .collect::<Result<_, _>>()?;

        let mut parts = Vec::with_capacity(columns.len() * PER_COLUMN);
        for (column, _, _) in &columns {
            let c = quoted(column)?;
            let v = format!("CAST({c} AS VARCHAR)");
            // Empty as decision 0033 says: NULL, or blank (U+00A0 too).
            let empty = format!("({v} IS NULL OR regexp_full_match({v}, '[\\s\\x{{00A0}}]*'))");
            parts.push(format!("count(*) FILTER (WHERE {empty})"));
            parts.push(format!(
                "count(DISTINCT CASE WHEN NOT {empty} THEN {c} END)"
            ));
            parts.push(format!("min(length({v})) FILTER (WHERE NOT {empty})"));
            parts.push(format!("max(length({v})) FILTER (WHERE NOT {empty})"));
            for (_, pattern) in patterns.all() {
                parts.push(format!(
                    "count(*) FILTER (WHERE regexp_full_match({v}, {}))",
                    quote_text(pattern).map_err(invalid)?
                ));
            }
        }
        let sql = format!(
            "WITH s AS (SELECT * FROM src.{}.{} LIMIT {sample}) SELECT count(*), {} FROM s",
            quoted(schema)?,
            quoted(name)?,
            parts.join(", ")
        );
        let width = 1 + columns.len() * PER_COLUMN;
        let row: Vec<Option<i64>> = self.con.query_row(&sql, [], |r| {
            (0..width).map(|i| r.get::<_, Option<i64>>(i)).collect()
        })?;
        let n = |i: usize| u64::try_from(row[i].unwrap_or(0)).unwrap_or(0);
        let length = |i: usize| row[i].and_then(|l| u32::try_from(l).ok());
        let profiles = columns
            .into_iter()
            .enumerate()
            .map(|(i, (column, declared_type, nullable))| {
                let at = 1 + i * PER_COLUMN;
                ColumnProfile {
                    primary_key: key.contains(&column),
                    name: column,
                    declared_type,
                    nullable,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tables_profile_is_its_metadata_and_its_samples_counts() {
        let runner = Runner::new().unwrap();
        // A DuckDB database standing for an attached source, synthetic rows.
        runner
            .con
            .execute_batch(
                "ATTACH ':memory:' AS src;
                 CREATE SCHEMA src.public;
                 CREATE TABLE src.public.clients (id INTEGER PRIMARY KEY, email VARCHAR, opened DATE);
                 INSERT INTO src.public.clients SELECT g,
                   CASE WHEN g % 4 = 0 THEN NULL WHEN g % 4 = 1 THEN chr(160) ELSE 'c' || g || '@example.org' END,
                   DATE '2020-01-01' + CAST(g AS INTEGER) FROM range(1, 101) t(g);",
            )
            .unwrap();
        let (sampled, columns) = runner
            .profile_source("public.clients", 40, &Patterns::default())
            .unwrap();
        assert_eq!(sampled, 40);
        let names: Vec<_> = columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["id", "email", "opened"]);
        let id = &columns[0];
        assert!(id.primary_key && !id.nullable);
        assert_eq!((id.empty, id.distinct), (0, 40));
        let email = &columns[1];
        // A quarter NULL and a quarter U+00A0: both empty.
        assert_eq!(
            (email.empty, email.distinct, email.looks.email),
            (20, 20, 20)
        );
        assert_eq!(columns[2].declared_type, "DATE");
        assert!(runner
            .profile_source("public.nothing", 10, &Patterns::default())
            .is_err());
        assert!(runner.takes_pattern(r"\d{5,12}") && !runner.takes_pattern("(unclosed"));
    }
}
