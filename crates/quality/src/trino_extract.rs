//! An extraction on Trino (decision 0048, point 3), pure: the statements
//! the worker runs, from its source's catalog into its bronze table in the
//! `lake` catalog, stamped with the cut and partitioned by it. On a first
//! extraction the table is created from the source's (`CREATE TABLE … AS
//! SELECT`); after, the cut's rows are deleted and inserted again, column by
//! column, so a retried extraction leaves the same rows. The namespace is
//! the application's (0037). Names are quoted; the cut is a checked date.

use serde_json::Value;

use crate::cuts::SourceColumn;
use crate::trino_catalog::catalog_name;

/// The static catalog of the lake in Trino: the main tenant's warehouse,
/// since Trino serves an on-premise installation, which has one tenant.
pub const LAKE: &str = "lake";

/// Why an extraction cannot be written as statements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractError {
    /// The cut is not a `YYYY-MM-DD` date.
    Cut,
    /// The source's table is not `schema.table`.
    Table,
}

/// One extraction: the source's table into `<dataset>_bronze.<table>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extraction {
    source_schema: String,
    source_table: String,
    catalog: String,
    namespace: String,
    table: String,
    cut: String,
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

fn is_date(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

impl Extraction {
    /// `source` (its name), its `table` as the mapping names it
    /// (`schema.table`), the `dataset`, its bronze `table` and the `cut`.
    pub fn new(
        source: &str,
        source_table: &str,
        dataset: &str,
        table: &str,
        cut: &str,
    ) -> Result<Self, ExtractError> {
        if !is_date(cut) {
            return Err(ExtractError::Cut);
        }
        let (schema, name) = source_table
            .split_once('.')
            .filter(|(s, t)| !s.is_empty() && !t.is_empty())
            .ok_or(ExtractError::Table)?;
        Ok(Self {
            source_schema: schema.to_string(),
            source_table: name.to_string(),
            catalog: catalog_name(source),
            namespace: format!("{dataset}_bronze"),
            table: table.to_string(),
            cut: cut.to_string(),
        })
    }

    fn source(&self) -> String {
        format!(
            "{}.{}.{}",
            self.catalog,
            quoted(&self.source_schema),
            quoted(&self.source_table)
        )
    }

    fn bronze(&self) -> String {
        format!("{LAKE}.{}.{}", quoted(&self.namespace), quoted(&self.table))
    }

    /// The source table's columns and Trino's types, in order.
    pub fn columns(&self) -> String {
        format!(
            "SELECT column_name, data_type FROM {}.information_schema.columns \
             WHERE table_schema = {} AND table_name = {} ORDER BY ordinal_position",
            self.catalog,
            literal(&self.source_schema.to_lowercase()),
            literal(&self.source_table.to_lowercase()),
        )
    }

    /// Whether the bronze table is there: one row with its count.
    pub fn exists(&self) -> String {
        format!(
            "SELECT count(*) FROM {LAKE}.information_schema.tables \
             WHERE table_schema = {} AND table_name = {}",
            literal(&self.namespace.to_lowercase()),
            literal(&self.table.to_lowercase()),
        )
    }

    /// The first extraction: the table made from the source's, with `cut`.
    pub fn create(&self) -> String {
        format!(
            "CREATE TABLE {} WITH (partitioning = ARRAY['cut']) AS \
             SELECT s.*, DATE '{}' AS cut FROM {} s",
            self.bronze(),
            self.cut,
            self.source()
        )
    }

    /// How many rows the cut has: one row with its count.
    pub fn cut_rows(&self) -> String {
        format!(
            "SELECT count(*) FROM {} WHERE cut = DATE '{}'",
            self.bronze(),
            self.cut
        )
    }

    /// The cut's rows, gone before they are written again. Only when it has
    /// some: the lake's catalog refuses a commit that deletes nothing
    /// (Verification of 0048).
    pub fn delete(&self) -> String {
        format!(
            "DELETE FROM {} WHERE cut = DATE '{}'",
            self.bronze(),
            self.cut
        )
    }

    /// The cut's rows written again, column by column.
    pub fn insert(&self, columns: &[SourceColumn]) -> String {
        let names: Vec<String> = columns.iter().map(|c| quoted(&c.name)).collect();
        let names = names.join(", ");
        format!(
            "INSERT INTO {} ({names}, cut) SELECT {names}, DATE '{}' FROM {}",
            self.bronze(),
            self.cut,
            self.source()
        )
    }

    /// The bronze table's last snapshot: the one this extraction committed.
    pub fn snapshot(&self) -> String {
        format!(
            "SELECT CAST(snapshot_id AS varchar) FROM {LAKE}.{}.{} \
             ORDER BY committed_at DESC LIMIT 1",
            quoted(&self.namespace),
            quoted(&format!("{}$snapshots", self.table))
        )
    }
}

/// The rows of [`Extraction::columns`]; a row that is not two texts is
/// skipped.
pub fn source_columns(rows: &[Vec<Value>]) -> Vec<SourceColumn> {
    rows.iter()
        .filter_map(|r| {
            Some(SourceColumn {
                name: r.first()?.as_str()?.to_string(),
                data_type: r.get(1)?.as_str()?.to_string(),
            })
        })
        .collect()
}

/// The single number a count, a `CREATE TABLE … AS` or an `INSERT` answers.
pub fn number(rows: &[Vec<Value>]) -> Option<i64> {
    rows.first()?.first()?.as_i64()
}

/// The single text a snapshot's statement answers.
pub fn text(rows: &[Vec<Value>]) -> Option<String> {
    rows.first()?.first()?.as_str().map(str::to_string)
}
