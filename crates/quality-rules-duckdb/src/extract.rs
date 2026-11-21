//! An extraction on the embedded DuckDB (decisions 0036 and 0037): the
//! source attached read-only through its driver's extension, before the
//! lake and its lockdown (`tools/verify/lake_extract.py`: once locked, the
//! attached source still reads, and nothing new can be attached); its
//! table read whole into the dataset's bronze table, which the first
//! extraction creates, partitioned by `cut`. Only names, types and counts
//! come out, never a value; an engine error may quote one, so the worker
//! keeps only the stage it happened in.

use std::collections::BTreeMap;

use quality_rules_sql::quote_text;

use crate::{invalid, quoted, Error, Runner};

/// A source's connection, as its job got it. The secrets never show in
/// `Debug`.
pub struct SourceConnection<'a> {
    /// `postgres`, `mysql`, `sqlserver` or `oracle` (`Runner::attach_source`).
    pub driver: &'a str,
    pub fields: &'a BTreeMap<String, String>,
    pub secrets: &'a BTreeMap<String, String>,
    /// The driver's default, when `fields` has no `port`.
    pub default_port: Option<u16>,
}

impl std::fmt::Debug for SourceConnection<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceConnection")
            .field("driver", &self.driver)
            .finish_non_exhaustive()
    }
}

/// A source column: its name and its type as bronze keeps it.
pub type Column = (String, String);

/// The type bronze keeps for a source column's type (decision 0077 point
/// 8): SQL Server's text aliases as `VARCHAR`, `TIMESTAMP_NS` (its
/// `DATETIME2`) as `TIMESTAMP`, which every Iceberg format version takes;
/// any other type, Oracle's `DATE` read as `TIMESTAMP` included, as it is.
pub(crate) fn bronze_type(ty: &str) -> String {
    let t = ty.to_ascii_uppercase();
    if t.starts_with("MSSQL_VARCHAR") || t.starts_with("MSSQL_NVARCHAR") {
        "VARCHAR".into()
    } else if t == "TIMESTAMP_NS" {
        "TIMESTAMP".into()
    } else {
        ty.to_string()
    }
}

/// `src."schema"."table"`, from the mapping's one or two parts.
pub(crate) fn source_table(table: &str) -> Result<String, Error> {
    let parts: Vec<&str> = table.split('.').collect();
    if parts.is_empty() || parts.len() > 2 {
        return Err(invalid(format!("a source table of {} parts", parts.len())));
    }
    let quoted: Result<Vec<String>, Error> = parts.iter().map(|p| quoted(p)).collect();
    Ok(format!("src.{}", quoted?.join(".")))
}

fn bronze_table(namespace: &str, table: &str) -> Result<String, Error> {
    Ok(format!("lake.{}.{}", quoted(namespace)?, quoted(table)?))
}

impl Runner {
    /// The source table's columns, in order, with the types bronze keeps.
    pub fn source_columns(&self, table: &str) -> Result<Vec<Column>, Error> {
        let mut stmt = self
            .con
            .prepare(&format!("DESCRIBE SELECT * FROM {}", source_table(table)?))?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, bronze_type(&r.get::<_, String>(1)?)))
        })?;
        rows.collect()
    }

    /// Whether a table of the lake (bronze or silver) is there.
    pub fn table_exists(&self, namespace: &str, table: &str) -> Result<bool, Error> {
        let n: i64 = self.con.query_row(
            "SELECT count(*) FROM duckdb_tables() WHERE database_name = 'lake' \
             AND schema_name = ? AND table_name = ?",
            [namespace, table],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Creates the bronze table with the source's columns and `cut`,
    /// partitioned by `cut` (decision 0037).
    pub fn create_bronze(
        &self,
        namespace: &str,
        table: &str,
        columns: &[Column],
    ) -> Result<(), Error> {
        if columns.iter().any(|(name, _)| name == "cut") {
            return Err(invalid("the source has a column named cut"));
        }
        let mut defs = Vec::with_capacity(columns.len() + 1);
        for (name, ty) in columns {
            defs.push(format!("{} {ty}", quoted(name)?));
        }
        defs.push("cut DATE".into());
        // The namespace is the application's (decision 0037).
        self.con.execute_batch(&format!(
            "CREATE TABLE {} ({}) PARTITIONED BY (cut);",
            bronze_table(namespace, table)?,
            defs.join(", ")
        ))
    }

    /// Replaces the cut's rows with the source table's, in one transaction
    /// (decision 0036, point 6): the rows written and the table's snapshot
    /// after the commit.
    pub fn replace_cut(
        &self,
        namespace: &str,
        table: &str,
        source: &str,
        columns: &[Column],
        cut: &str,
    ) -> Result<(i64, String), Error> {
        let bronze = bronze_table(namespace, table)?;
        let cut = quote_text(cut).map_err(invalid)?;
        let (mut names, mut values) = (Vec::new(), Vec::new());
        for (name, ty) in columns {
            names.push(quoted(name)?);
            values.push(format!("CAST({} AS {ty})", quoted(name)?));
        }
        let (names, values) = (names.join(", "), values.join(", "));
        self.con.execute_batch(&format!(
            "BEGIN;\n\
             DELETE FROM {bronze} WHERE cut = CAST({cut} AS DATE);\n\
             INSERT INTO {bronze} ({names}, cut) SELECT {values}, CAST({cut} AS DATE) FROM {};\n\
             COMMIT;",
            source_table(source)?
        ))?;
        let rows: i64 = self.con.query_row(
            &format!("SELECT count(*) FROM {bronze} WHERE cut = CAST({cut} AS DATE)"),
            [],
            |r| r.get(0),
        )?;
        Ok((rows, self.latest_snapshot(&bronze)?.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_table_has_one_or_two_quoted_parts() {
        assert_eq!(source_table("clients").unwrap(), "src.\"clients\"");
        assert_eq!(
            source_table("public.clients").unwrap(),
            "src.\"public\".\"clients\""
        );
        assert!(source_table("db.public.clients").is_err());
    }

    #[test]
    fn bronze_keeps_text_aliases_as_varchar_and_timestamps_in_microseconds() {
        assert_eq!(bronze_type("MSSQL_VARCHAR(10)"), "VARCHAR");
        assert_eq!(bronze_type("MSSQL_NVARCHAR(60)"), "VARCHAR");
        assert_eq!(bronze_type("TIMESTAMP_NS"), "TIMESTAMP");
        assert_eq!(bronze_type("TIMESTAMP"), "TIMESTAMP", "Oracle's DATE");
        assert_eq!(bronze_type("DECIMAL(12,2)"), "DECIMAL(12,2)");
        let runner = Runner::new().unwrap();
        runner
            .con
            .execute_batch(
                "ATTACH ':memory:' AS src;
                 CREATE TABLE src.main.clients (id INTEGER, updated TIMESTAMP_NS, code VARCHAR);",
            )
            .unwrap();
        let columns = runner.source_columns("main.clients").unwrap();
        let types: Vec<_> = columns.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(types, ["INTEGER", "TIMESTAMP", "VARCHAR"]);
    }

    #[test]
    fn a_source_connection_never_prints_its_secrets() {
        let secrets: BTreeMap<String, String> =
            [("password".to_string(), "synthetic-secret".to_string())].into();
        let fields = BTreeMap::new();
        let c = SourceConnection {
            driver: "postgres",
            fields: &fields,
            secrets: &secrets,
            default_port: Some(5432),
        };
        assert!(!format!("{c:?}").contains("synthetic-secret"));
    }
}
