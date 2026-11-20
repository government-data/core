//! What a maintain job asks DuckDB of a dataset's tables (decision 0039):
//! which tables its bronze namespace has, a table's data files (as of a
//! snapshot), its snapshots, and compacting its small files
//! (`iceberg_rewrite_data_files`). Names and counts only;
//! `tools/verify/lake_maintain.py` shows each on DuckDB 2.0.0.

use quality_rules_sql::quote_text;

use crate::{invalid, quoted, Error, Runner};

fn table(namespace: &str, name: &str) -> Result<String, Error> {
    Ok(format!("lake.{}.{}", quoted(namespace)?, quoted(name)?))
}

/// A table's snapshot: its id and when it was made (milliseconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    pub id: i64,
    pub at_ms: i64,
}

impl Runner {
    /// The tables of `namespace` in the attached lake, by name.
    pub fn lake_tables(&self, namespace: &str) -> Result<Vec<String>, Error> {
        let mut stmt = self.con.prepare(
            "SELECT table_name FROM duckdb_tables() \
             WHERE database_name = 'lake' AND schema_name = ? ORDER BY table_name",
        )?;
        let names = stmt.query_map([namespace], |r| r.get(0))?;
        names.collect()
    }

    /// The table's data files, as of `snapshot` or now.
    pub fn data_files(
        &self,
        namespace: &str,
        name: &str,
        snapshot: Option<i64>,
    ) -> Result<Vec<String>, Error> {
        let at = snapshot.map_or(String::new(), |s| format!(", snapshot_from_id => {s}"));
        let mut stmt = self.con.prepare(&format!(
            "SELECT DISTINCT file_path FROM iceberg_metadata({}{at}) \
             WHERE manifest_content = 'DATA' AND status != 'DELETED'",
            table(namespace, name)?
        ))?;
        let files = stmt.query_map([], |r| r.get(0))?;
        files.collect()
    }

    /// Every data file a snapshot's manifests name, their `DELETED` entries
    /// too: RustFS refuses a later commit if one of those is gone, so none
    /// of them is an orphan (decision 0039).
    pub fn named_files(
        &self,
        namespace: &str,
        name: &str,
        snapshot: i64,
    ) -> Result<Vec<String>, Error> {
        let mut stmt = self.con.prepare(&format!(
            "SELECT DISTINCT file_path FROM iceberg_metadata({}, snapshot_from_id => {snapshot}) \
             WHERE manifest_content = 'DATA'",
            table(namespace, name)?
        ))?;
        let files = stmt.query_map([], |r| r.get(0))?;
        files.collect()
    }

    /// The table's snapshots, oldest first; the last is the current one.
    pub fn table_snapshots(&self, namespace: &str, name: &str) -> Result<Vec<Snapshot>, Error> {
        let mut stmt = self.con.prepare(&format!(
            "SELECT snapshot_id, epoch_ms(timestamp_ms) FROM iceberg_snapshots({}) \
             ORDER BY sequence_number",
            table(namespace, name)?
        ))?;
        let snapshots = stmt.query_map([], |r| {
            Ok(Snapshot {
                id: r.get(0)?,
                at_ms: r.get(1)?,
            })
        })?;
        snapshots.collect()
    }

    /// Rewrites the table's small files into large ones: the same rows, a
    /// new snapshot. The files rewritten and added.
    pub fn compact(&self, namespace: &str, name: &str) -> Result<(i64, i64), Error> {
        let target = quote_text(&format!("lake.{namespace}.{name}")).map_err(invalid)?;
        self.con.query_row(
            &format!(
                "SELECT coalesce(sum(rewritten_data_files), 0), coalesce(sum(added_data_files), 0) \
                 FROM iceberg_rewrite_data_files({target}, min_input_files => 2)"
            ),
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
    }
}
