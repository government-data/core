//! A console statement over the attached lake (decision 0054), already
//! wrapped by `quality::console::wrapped`: its columns and its rows as
//! text. Values come out here: the caller reports them to the API, which
//! masks and audits them, and never logs them.

use crate::{Error, Runner};

/// Columns, and rows of values as text (`None` where `NULL`).
pub type Rows = (Vec<String>, Vec<Vec<Option<String>>>);

impl Runner {
    /// Runs `wrapped` on the locked-down connection, every column read as
    /// text under its own name.
    pub fn console(&self, wrapped: &str) -> Result<Rows, Error> {
        let mut stmt = self.con.prepare(&format!(
            "SELECT CAST(COLUMNS(*) AS VARCHAR) FROM ({wrapped}) AS r"
        ))?;
        let mut rows = stmt.query([])?;
        let mut read = Vec::new();
        let mut width = 0;
        while let Some(row) = rows.next()? {
            width = row.as_ref().column_count();
            read.push(
                (0..width)
                    .map(|i| row.get::<_, Option<String>>(i))
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        drop(rows);
        let columns = stmt.column_names();
        debug_assert!(read.is_empty() || columns.len() == width);
        Ok((columns, read))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_statement_comes_back_as_text_under_its_names() {
        let runner = Runner::new().unwrap();
        let (columns, rows) = runner
            .console(
                "SELECT * FROM (SELECT 1 AS n, NULL AS z, DATE '2026-01-02' AS d\n) AS q LIMIT 201",
            )
            .unwrap();
        assert_eq!(columns, ["n", "z", "d"]);
        assert_eq!(
            rows,
            [[Some("1".to_string()), None, Some("2026-01-02".to_string())]]
        );
        let (columns, rows) = runner
            .console("SELECT * FROM (SELECT 1 AS n WHERE false\n) AS q LIMIT 201")
            .unwrap();
        assert_eq!((columns.len(), rows.len()), (1, 0));
        assert!(runner
            .console("SELECT * FROM (ATTACH 'x'\n) AS q LIMIT 201")
            .is_err());
    }
}
