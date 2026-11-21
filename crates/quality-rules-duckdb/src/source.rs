//! A source attached for a job (decisions 0036 and 0077): read-only, through
//! its driver's DuckDB extension, `src` to every reader (listing, profile,
//! records, extraction). Every extension loads from the extension
//! directory the worker's image fills, and nothing is installed at run time
//! (decision 0077 point 8, `tools/verify/duckdb_source_*.sh`): without one
//! set, no source attaches.
//!
//! What is the server's and what is DuckDB's (decision 0077 point 8): SQL
//! Server is attached with `remote_pushdown false`, so aggregates,
//! `DISTINCT` and joins are DuckDB's while filters follow its collation,
//! its certificate verified unless the source's `trust_server_certificate`
//! is set;
//! Oracle with `oracle_scanner`'s aggregate and join pushdown off, and its
//! owner as the attachment's `main`.

use std::collections::BTreeMap;

use quality::source::flag;
use quality_rules_sql::quote_text;

use crate::{invalid, Error, Runner, SourceConnection};

/// The DuckDB extension that reads a driver's sources.
fn extension(driver: &str) -> Result<&'static str, Error> {
    match driver {
        "postgres" => Ok("postgres"),
        "mysql" => Ok("mysql"),
        "sqlserver" => Ok("mssql"),
        "oracle" => Ok("oracle_scanner"),
        other => Err(invalid(format!("no DuckDB driver for {other}"))),
    }
}

impl Runner {
    /// Loads every extension from `dir` from now on, and never installs or
    /// loads one by itself: the worker's image puts them there when it is
    /// built (decision 0077 point 8). Before [`Runner::attach_source`].
    pub fn set_extension_directory(&mut self, dir: &str) -> Result<(), Error> {
        self.con.execute_batch(&format!(
            "SET extension_directory = {};\n\
             SET autoinstall_known_extensions = false;\n\
             SET autoload_known_extensions = false;",
            quote_text(dir).map_err(invalid)?
        ))?;
        self.offline = true;
        Ok(())
    }

    /// Attaches the source as `src`, read-only; an Oracle source with the
    /// connected user as its owner. Before [`Runner::attach_lake`], which
    /// locks the connection.
    pub fn attach_source(&mut self, source: &SourceConnection) -> Result<(), Error> {
        self.attach_source_as(source, None)
    }

    /// [`Runner::attach_source`], an Oracle source reading the tables of
    /// `owner` (an Oracle schema) as `src.main`; `None` is the connected
    /// user. Only Oracle sources take an owner.
    pub fn attach_source_as(
        &mut self,
        source: &SourceConnection,
        owner: Option<&str>,
    ) -> Result<(), Error> {
        let sql = attach_sql(source, owner, self.offline)?;
        self.con.execute_batch(&sql)
    }
}

/// The statements that load the extension and attach the source.
fn attach_sql(
    source: &SourceConnection,
    owner: Option<&str>,
    offline: bool,
) -> Result<String, Error> {
    let ext = extension(source.driver)?;
    if owner.is_some() && source.driver != "oracle" {
        return Err(invalid(format!(
            "a {} source takes no owner",
            source.driver
        )));
    }
    if !offline {
        return Err(invalid(format!(
            "{ext} loads only from an extension directory"
        )));
    }
    let load = format!("LOAD {ext};");
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
    let host = q(&get(source.fields, "host")?)?;
    let user = get(source.secrets, "user")?;
    let password = q(&get(source.secrets, "password")?)?;
    let attach = match source.driver {
        "sqlserver" => format!(
            "CREATE OR REPLACE SECRET src_job (TYPE mssql, host {host}, port {port}, \
             database {}, user {}, password {password}, trust_server_certificate {});\n\
             ATTACH '' AS src (TYPE mssql, SECRET src_job, READ_ONLY, remote_pushdown false);",
            q(&get(source.fields, "database")?)?,
            q(&user)?,
            flag(source.fields, "trust_server_certificate"),
        ),
        "oracle" => {
            // Oracle keeps an unquoted user name in upper case.
            let owner = owner.map_or_else(|| user.to_uppercase(), str::to_string);
            format!(
                "SET oracle_aggregate_pushdown = false;\n\
                 SET oracle_join_pushdown = false;\n\
                 CREATE OR REPLACE SECRET src_job (TYPE oracle, HOST {host}, PORT {port}, \
                 SERVICE_NAME {}, USER {}, PASSWORD {password});\n\
                 ATTACH 'src_job' AS src (TYPE oracle_scanner, SCHEMA {}, READ_ONLY);",
                q(&get(source.fields, "service")?)?,
                q(&user)?,
                q(&owner)?,
            )
        }
        _ => format!(
            "CREATE OR REPLACE SECRET src_job (TYPE {ext}, HOST {host}, PORT {port}, \
             DATABASE {}, USER {}, PASSWORD {password});\n\
             ATTACH '' AS src (TYPE {ext}, SECRET src_job, READ_ONLY);",
            q(&get(source.fields, "database")?)?,
            q(&user)?,
        ),
    };
    Ok(format!("{load}\n{attach}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// The statements for a synthetic source of `driver`.
    fn sql(driver: &str, owner: Option<&str>, offline: bool) -> Result<String, Error> {
        let fields = map(&[
            ("host", "db.example"),
            ("database", "core"),
            ("service", "FREEPDB1"),
        ]);
        let secrets = map(&[("user", "qs_reader"), ("password", "synthetic")]);
        let source = SourceConnection {
            driver,
            fields: &fields,
            secrets: &secrets,
            default_port: Some(1000),
        };
        attach_sql(&source, owner, offline)
    }

    #[test]
    fn sql_server_loads_offline_with_its_pushdown_off() {
        let s = sql("sqlserver", None, true).unwrap();
        assert!(s.starts_with("LOAD mssql;"), "{s}");
        assert!(s.contains("TYPE mssql, host 'db.example', port 1000, database 'core'"));
        assert!(s.contains("READ_ONLY, remote_pushdown false)"));
        assert!(
            s.contains("trust_server_certificate false)"),
            "verified unless set"
        );
        assert!(!s.contains("INSTALL"));
    }

    #[test]
    fn sql_server_trusts_its_certificate_only_when_the_source_says_so() {
        let fields = map(&[
            ("host", "db.example"),
            ("database", "core"),
            ("trust_server_certificate", "true"),
        ]);
        let secrets = map(&[("user", "qs_reader"), ("password", "synthetic")]);
        let source = SourceConnection {
            driver: "sqlserver",
            fields: &fields,
            secrets: &secrets,
            default_port: Some(1433),
        };
        let s = attach_sql(&source, None, true).unwrap();
        assert!(
            s.contains("password 'synthetic', trust_server_certificate true)"),
            "{s}"
        );
    }

    #[test]
    fn oracle_reads_its_owner_as_main_with_aggregates_and_joins_in_duckdb() {
        let s = sql("oracle", None, true).unwrap();
        assert!(s.starts_with("LOAD oracle_scanner;"), "{s}");
        assert!(s.contains("SET oracle_aggregate_pushdown = false;"));
        assert!(s.contains("SET oracle_join_pushdown = false;"));
        assert!(s.contains("SERVICE_NAME 'FREEPDB1'"));
        assert!(
            s.contains("SCHEMA 'QS_READER', READ_ONLY)"),
            "the connected user"
        );
        assert!(!s.contains("INSTALL"));
        let s = sql("oracle", Some("QS_APP"), true).unwrap();
        assert!(s.contains("SCHEMA 'QS_APP', READ_ONLY)"));
    }

    #[test]
    fn postgres_and_mysql_load_offline_too() {
        for driver in ["postgres", "mysql"] {
            let s = sql(driver, None, true).unwrap();
            assert!(s.starts_with(&format!("LOAD {driver};")), "{s}");
            assert!(!s.contains("INSTALL"));
            assert!(!s.contains("trust_server_certificate"));
        }
    }

    #[test]
    fn what_cannot_be_attached_is_refused() {
        for driver in ["postgres", "mysql", "sqlserver", "oracle"] {
            assert!(
                sql(driver, None, false).is_err(),
                "{driver}: never installed"
            );
        }
        assert!(sql("postgres", Some("HR"), true).is_err(), "no owner");
        assert!(sql("sqlserver", Some("dbo"), true).is_err(), "no owner");
        assert!(sql("openedge", None, true).is_err());
    }

    #[test]
    fn with_an_extension_directory_nothing_is_installed() {
        let dir = std::env::temp_dir().join(format!("qr-duckdb-ext-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut runner = Runner::new().unwrap();
        runner
            .set_extension_directory(dir.to_str().unwrap())
            .unwrap();
        let auto: (bool, bool) = runner
            .con
            .query_row(
                "SELECT current_setting('autoinstall_known_extensions'), \
                 current_setting('autoload_known_extensions')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(auto, (false, false));
        let fields = map(&[("host", "db.example"), ("database", "core")]);
        let secrets = map(&[("user", "qs_reader"), ("password", "synthetic")]);
        let source = SourceConnection {
            driver: "sqlserver",
            fields: &fields,
            secrets: &secrets,
            default_port: Some(1433),
        };
        let e = runner.attach_source(&source).unwrap_err().to_string();
        assert!(!e.contains("synthetic"), "{e}");
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "nothing fetched"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
