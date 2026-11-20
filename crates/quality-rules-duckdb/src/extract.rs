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
    /// `postgres` or `mysql`: the DuckDB extension of the same name.
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

/// A source column: its name and its type as DuckDB reads it.
pub type Column = (String, String);

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
    /// Attaches the source as `src`, read-only. Before [`Runner::attach_lake`],
    /// which locks the connection.
    pub fn attach_source(&mut self, source: &SourceConnection) -> Result<(), Error> {
        let extension = match source.driver {
            "postgres" => "postgres",
            "mysql" => "mysql",
            other => return Err(invalid(format!("no DuckDB driver for {other}"))),
        };
        let q = |t: &str| quote_text(t).map_err(invalid);
        let get = |m: &BTreeMap<String, String>, k: &str| {
            m.get(k)
                .cloned()
                .ok_or_else(|| invalid(format!("the source has no {k}")))
        };
        let port = match source.fields.get("port") {
            Some(p) => p.parse::<u16>().map_err(invalid)?,
            None => source
                .default_port
                .ok_or_else(|| invalid("the source has no port"))?,
        };
        self.con.execute_batch(&format!(
            "INSTALL {extension}; LOAD {extension};\n\
             CREATE OR REPLACE SECRET src_job (TYPE {extension}, HOST {}, PORT {port}, \
             DATABASE {}, USER {}, PASSWORD {});\n\
             ATTACH '' AS src (TYPE {extension}, SECRET src_job, READ_ONLY);",
            q(&get(source.fields, "host")?)?,
            q(&get(source.fields, "database")?)?,
            q(&get(source.secrets, "user")?)?,
            q(&get(source.secrets, "password")?)?,
        ))
    }

    /// The source table's columns, in order, with their types.
    pub fn source_columns(&self, table: &str) -> Result<Vec<Column>, Error> {
        let mut stmt = self
            .con
            .prepare(&format!("DESCRIBE SELECT * FROM {}", source_table(table)?))?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
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
        let mut names = Vec::with_capacity(columns.len());
        for (name, _) in columns {
            names.push(quoted(name)?);
        }
        let names = names.join(", ");
        self.con.execute_batch(&format!(
            "BEGIN;\n\
             DELETE FROM {bronze} WHERE cut = CAST({cut} AS DATE);\n\
             INSERT INTO {bronze} ({names}, cut) SELECT {names}, CAST({cut} AS DATE) FROM {};\n\
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
