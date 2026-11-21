//! A tenant's lake, attached for a job (decisions 0017 and 0030): its
//! warehouse through DuckDB's `iceberg` extension, with the job's own
//! temporary credentials, and then the connection locked down as decision
//! 0033 verified: only that warehouse's catalog and bucket stay reachable,
//! no local file, no extension, no later setting.

use quality::layers;
use quality::mapping::Mapping;
use quality_rules_sql::quote_text;

use crate::{invalid, quoted, Error, Runner};

/// Where the warehouse is and the job's credentials for it. The secret and
/// the token never show in `Debug`.
pub struct LakeConnection<'a> {
    /// The lake's S3 endpoint, `https://host[:port]`; its Iceberg catalog
    /// is under `/iceberg`.
    pub endpoint: &'a str,
    pub warehouse: &'a str,
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub session_token: &'a str,
    /// A CA to trust besides the system's (development).
    pub ca_file: Option<&'a str>,
}

impl std::fmt::Debug for LakeConnection<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LakeConnection")
            .field("endpoint", &self.endpoint)
            .field("warehouse", &self.warehouse)
            .finish_non_exhaustive()
    }
}

/// Which silver snapshot a read takes (decision 0061): the one its
/// extraction wrote, or the table's current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilverAt {
    Pinned(i64),
    Current,
}

/// A dataset's cut as it was read: the snapshot, of silver's table or of
/// bronze's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LakeRead {
    pub snapshot: String,
    pub silver: bool,
}

impl Runner {
    /// Installs the extensions a job needs in DuckDB's extension
    /// directory, so it loads them without fetching anything: the lake's
    /// (`iceberg`, the `avro` it loads, `httpfs`) and each source driver's
    /// ([`Runner::attach_source`]: `postgres`, `mysql`, and the community's
    /// `mssql` and `oracle_scanner`). The worker's image does it once, when
    /// it is built, and checks each source extension's hash (decision 0077
    /// point 8).
    pub fn install_extensions() -> Result<(), Error> {
        Runner::new()?.con.execute_batch(
            "INSTALL avro; INSTALL iceberg; INSTALL httpfs; INSTALL postgres; INSTALL mysql;\n\
             INSTALL mssql FROM community; INSTALL oracle_scanner FROM community;",
        )
    }

    /// Attaches the warehouse as `lake` and locks the connection down.
    /// Nothing outside the warehouse can be read after it, so it comes
    /// before any rule runs.
    pub fn attach_lake(&mut self, lake: &LakeConnection) -> Result<(), Error> {
        let (ssl, host) = match lake.endpoint.trim_end_matches('/').split_once("://") {
            Some(("https", host)) => (true, host),
            Some(("http", host)) => (false, host),
            _ => return Err(invalid(format!("lake endpoint {}", lake.endpoint))),
        };
        let q = |t: &str| quote_text(t).map_err(invalid);
        let base = lake.endpoint.trim_end_matches('/');
        let mut sql = String::from("INSTALL avro; LOAD avro; INSTALL iceberg; LOAD iceberg; INSTALL httpfs; LOAD httpfs;\n");
        if let Some(ca) = lake.ca_file {
            sql += &format!("SET ca_cert_file = {};\n", q(ca)?);
        }
        sql += &format!(
            "CREATE SECRET lake_job (TYPE s3, KEY_ID {}, SECRET {}, SESSION_TOKEN {}, REGION 'us-east-1', \
             ENDPOINT {}, URL_STYLE 'path', USE_SSL {ssl});\n",
            q(lake.access_key)?,
            q(lake.secret_key)?,
            q(lake.session_token)?,
            q(host)?,
        );
        sql += &format!(
            "ATTACH {} AS lake (TYPE iceberg, URI {}, AUTHORIZATION_TYPE 'sigv4', \
             SIGV4_SERVICE 's3', SIGV4_REGION 'us-east-1', SECRET lake_job);\n",
            q(lake.warehouse)?,
            q(&format!("{base}/iceberg"))?,
        );
        // Without the list, the lockdown also cuts the attached catalog.
        sql += &format!(
            "SET allowed_directories = [{}, {}];\n\
             SET enable_external_access = false;\n\
             SET lock_configuration = true;",
            q(&format!("{base}/iceberg/v1/{}/", lake.warehouse))?,
            q(&format!("s3://{}/", lake.warehouse))?,
        );
        self.con.execute_batch(&sql)
    }

    /// Loads a dataset's cut through its mapping from the attached lake:
    /// from its silver table at `silver` when silver holds the cut under
    /// this mapping (decision 0061, point 4), or else from its bronze table
    /// (`<dataset>_bronze.<source>`) typed as mapped, pinned to `snapshot`
    /// or the table's current one. What it read is what the run's manifest
    /// records, and what reads it again (decision 0035;
    /// `tools/verify/lake_manifests.py`).
    pub fn load_mapped_lake(
        &mut self,
        mapping: &Mapping,
        cut: &str,
        snapshot: Option<i64>,
        silver: Option<SilverAt>,
    ) -> Result<LakeRead, Error> {
        let (source, table, snapshot) = self.bronze_cut(mapping, cut, snapshot)?;
        if let Some(at) = silver {
            if let Some((select, table, snapshot)) = self.silver_cut(mapping, cut, &source, at)? {
                self.view(mapping, &select, &table)?;
                return Ok(LakeRead {
                    snapshot: snapshot.to_string(),
                    silver: true,
                });
            }
        }
        self.map(mapping, &source, &table)?;
        Ok(LakeRead {
            snapshot: snapshot.to_string(),
            silver: false,
        })
    }

    /// The bronze table's cut as the view `<dataset>__source`, pinned to
    /// `snapshot` or else the table's current one: the view's name, the
    /// table's and the snapshot read.
    pub(crate) fn bronze_cut(
        &self,
        mapping: &Mapping,
        cut: &str,
        snapshot: Option<i64>,
    ) -> Result<(String, String, i64), Error> {
        let (namespace, table) = layers::bronze(mapping);
        let table = format!("lake.{}.{}", quoted(&namespace)?, quoted(&table)?);
        let snapshot = match snapshot {
            Some(id) => id,
            None => self.latest_snapshot(&table)?,
        };
        let source = format!("{}__source", mapping.dataset.value);
        self.con.execute_batch(&format!(
            "CREATE OR REPLACE VIEW {} AS SELECT * FROM {table} AT (VERSION => {snapshot}) \
             WHERE cut = CAST({} AS DATE)",
            quoted(&source)?,
            quote_text(cut).map_err(invalid)?,
        ))?;
        Ok((source, table, snapshot))
    }

    /// A lake table's current snapshot.
    pub(crate) fn latest_snapshot(&self, table: &str) -> Result<i64, Error> {
        self.con.query_row(
            &format!(
                "SELECT snapshot_id FROM iceberg_snapshots({table}) \
                 ORDER BY sequence_number DESC LIMIT 1"
            ),
            [],
            |r| r.get(0),
        )
    }

    /// How many records a loaded dataset has.
    pub fn records(&self, dataset: &str) -> Result<u64, Error> {
        self.con.query_row(
            &format!("SELECT count(*) FROM {}", quoted(dataset)?),
            [],
            |r| r.get(0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_endpoint_needs_its_scheme_and_nothing_is_attached_without_it() {
        let mut runner = Runner::new().unwrap();
        let lake = LakeConnection {
            endpoint: "lake.example:9000",
            warehouse: "qd-tenant-a",
            access_key: "k",
            secret_key: "s",
            session_token: "t",
            ca_file: None,
        };
        assert!(runner.attach_lake(&lake).is_err());
        let shown = format!("{lake:?}");
        assert!(!shown.contains("secret_key") && !shown.contains("session_token"));
    }
}
