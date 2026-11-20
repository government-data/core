//! A console statement (decision 0054): checked and wrapped so that only
//! one query runs and no more than [`MAX_ROWS`] rows come back, and its
//! result as its worker reports it. Pure. What DuckDB refuses inside the
//! wrapper is in `tools/verify/lake_console_duckdb.py`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::rule_test::mask;

/// The most rows a result has; one more is read to know there are more.
pub const MAX_ROWS: usize = 200;
/// The longest statement taken, in characters.
pub const MAX_CHARS: usize = 20_000;

/// Why a statement is not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementError {
    Empty,
    TooLong,
    /// More than one statement (a `;` but at its end).
    Several,
}

/// The statement as the worker runs it: inside a query of its own, so that
/// anything else does not parse, and limited. The `)` goes on a line of its
/// own, out of a comment at the statement's end.
pub fn wrapped(sql: &str) -> Result<String, StatementError> {
    let sql = sql.trim().trim_end_matches(';').trim_end();
    if sql.is_empty() {
        return Err(StatementError::Empty);
    }
    if sql.chars().count() > MAX_CHARS {
        return Err(StatementError::TooLong);
    }
    if sql.contains(';') {
        return Err(StatementError::Several);
    }
    Ok(format!(
        "SELECT * FROM ({sql}\n) AS q LIMIT {}",
        MAX_ROWS + 1
    ))
}

/// What a `query` job read: its columns and rows as text (`null` where
/// empty), whether there were more, or the engine's message if it failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ConsoleResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub more: bool,
    /// The engine's message; it may quote a value.
    pub error: Option<String>,
}

impl ConsoleResult {
    /// At most [`MAX_ROWS`], each as wide as the columns.
    pub fn is_valid(&self) -> bool {
        self.rows.len() <= MAX_ROWS && self.rows.iter().all(|r| r.len() == self.columns.len())
    }

    /// For whoever may not see personal data: every value masked, and of
    /// the engine's message only its kind.
    #[must_use]
    pub fn masked(mut self) -> Self {
        for value in self.rows.iter_mut().flatten().flatten() {
            *value = mask(value);
        }
        self.error = self.error.as_deref().map(|e| error_kind(e).to_string());
        self
    }
}

/// The kind of an engine's message, which names no value: `syntax`,
/// `name` (a table or column that is not there), `conversion`,
/// `permission` or `engine`.
pub fn error_kind(message: &str) -> &'static str {
    if message.contains("Parser Error") {
        "syntax"
    } else if message.contains("Binder Error") || message.contains("Catalog Error") {
        "name"
    } else if message.contains("Conversion Error") || message.contains("Invalid Input") {
        "conversion"
    } else if message.contains("Permission Error") {
        "permission"
    } else {
        "engine"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_query_is_wrapped_and_limited() {
        assert_eq!(
            wrapped("  SELECT 1;  ").unwrap(),
            "SELECT * FROM (SELECT 1\n) AS q LIMIT 201"
        );
        // A comment at its end does not hide the wrapper's `)`.
        assert!(wrapped("SELECT 1 -- note")
            .unwrap()
            .contains("note\n) AS q"));
        assert_eq!(wrapped(" ; "), Err(StatementError::Empty));
        assert_eq!(
            wrapped("SELECT 1; DROP TABLE t"),
            Err(StatementError::Several)
        );
        assert_eq!(
            wrapped(&"x".repeat(MAX_CHARS + 1)),
            Err(StatementError::TooLong)
        );
    }

    #[test]
    fn a_masked_result_hides_values_and_the_engines_words() {
        let result = ConsoleResult {
            columns: vec!["n".into()],
            rows: vec![vec![Some("1234567".into())], vec![None]],
            more: false,
            error: Some("Conversion Error: Could not convert string 'x' to INT32".into()),
        };
        assert!(result.is_valid());
        let masked = result.masked();
        assert_eq!(masked.rows, vec![vec![Some("••••4567".into())], vec![None]]);
        assert_eq!(masked.error.as_deref(), Some("conversion"));
        assert_eq!(
            error_kind("Parser Error: syntax error at or near"),
            "syntax"
        );
        assert_eq!(
            error_kind("Catalog Error: Table with name x does not exist"),
            "name"
        );
    }
}
