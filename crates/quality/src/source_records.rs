//! A source table's records, read as a database client reads them (decision
//! 0049, point 3), pure: what is asked (a page of [`PAGE`] records, the
//! first ones, the last ones by a column, or page _n_ in an order), the page
//! a `records` job reports, masked for whoever may not see personal data,
//! and the statement Trino runs for a source on it. DuckDB reads the same
//! page through `quality-rules-duckdb`. Never more than a page at a time.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::rule_test::mask;
use crate::trino_catalog::catalog_name;

/// The records of a page.
pub const PAGE: u32 = 1000;
/// The furthest page asked: a million records into a table, then an order
/// that reaches the rest.
pub const MAX_PAGE: u32 = 999;

/// What is asked of a source's table: page `page` (from 0) of [`PAGE`]
/// records, in `order` if given (descending if `descending`: the last ones
/// first), else as the engine gives them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RecordsAsk {
    /// `schema.table`, as the source's listing names it.
    pub table: String,
    #[serde(default)]
    pub order: Option<String>,
    #[serde(default)]
    pub descending: bool,
    #[serde(default)]
    pub page: u32,
}

/// Why a page cannot be asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskError {
    /// The table is not `schema.table`.
    Table,
    /// Descending with no column to order by: there is no "last" without one.
    NoOrder,
    /// Past [`MAX_PAGE`].
    TooFar,
}

impl RecordsAsk {
    /// The table's schema and name.
    pub fn schema_and_table(&self) -> Result<(&str, &str), AskError> {
        self.table
            .split_once('.')
            .filter(|(s, t)| !s.is_empty() && !t.is_empty() && !t.contains('.'))
            .ok_or(AskError::Table)
    }

    /// Whether it can be asked: a table, an order for the last ones, a page
    /// within reach.
    pub fn check(&self) -> Result<(), AskError> {
        self.schema_and_table()?;
        if self.descending && self.order.is_none() {
            return Err(AskError::NoOrder);
        }
        if self.page > MAX_PAGE {
            return Err(AskError::TooFar);
        }
        Ok(())
    }

    /// How many records come before the page.
    pub fn offset(&self) -> u64 {
        u64::from(self.page) * u64::from(PAGE)
    }
}

/// A page of a source table's records, as the job reports it: the columns
/// in the table's order, each record's values as text (`None` where empty),
/// and whether there are more after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RecordsPage {
    pub source: String,
    pub table: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub more: bool,
}

impl RecordsPage {
    /// The page from the `PAGE + 1` records read: the last one only says
    /// there are more.
    pub fn new(
        source: &str,
        table: &str,
        columns: Vec<String>,
        mut rows: Vec<Vec<Option<String>>>,
    ) -> Self {
        let more = rows.len() > PAGE as usize;
        rows.truncate(PAGE as usize);
        Self {
            source: source.to_string(),
            table: table.to_string(),
            columns,
            rows,
            more,
        }
    }

    /// At most a page, each record as wide as the columns.
    pub fn is_valid(&self) -> bool {
        self.rows.len() <= PAGE as usize && self.rows.iter().all(|r| r.len() == self.columns.len())
    }

    /// Every value masked, for whoever may not see personal data: `••••`
    /// and its last four characters.
    #[must_use]
    pub fn masked(mut self) -> Self {
        for row in &mut self.rows {
            for value in row.iter_mut().flatten() {
                *value = mask(value);
            }
        }
        self
    }
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The statement Trino runs for `ask` on the source `source`'s catalog:
/// one record more than a page, to know whether there are more.
pub fn trino_statement(source: &str, ask: &RecordsAsk) -> Result<String, AskError> {
    ask.check()?;
    let (schema, table) = ask.schema_and_table()?;
    let order = ask.order.as_deref().map_or(String::new(), |c| {
        let way = if ask.descending { "DESC" } else { "ASC" };
        format!(" ORDER BY {} {way}", quoted(c))
    });
    Ok(format!(
        "SELECT * FROM {}.{}.{}{order} OFFSET {} LIMIT {}",
        catalog_name(source),
        quoted(schema),
        quoted(table),
        ask.offset(),
        PAGE + 1
    ))
}

fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The statement Trino runs for the names of the table's columns, in the
/// order `SELECT *` gives them: its answer has values only.
pub fn trino_columns(source: &str, ask: &RecordsAsk) -> Result<String, AskError> {
    let (schema, table) = ask.schema_and_table()?;
    Ok(format!(
        "SELECT column_name FROM {}.information_schema.columns \
         WHERE table_schema = {} AND table_name = {} ORDER BY ordinal_position",
        catalog_name(source),
        literal(&schema.to_lowercase()),
        literal(&table.to_lowercase()),
    ))
}
