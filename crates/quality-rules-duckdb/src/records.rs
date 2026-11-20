//! A page of a source table's records (decision 0049, point 3), read
//! through the attached source: the table's columns in order, each value
//! as text, one record more than a page to know whether there are more.
//! Values come out here: the API masks them for whoever may not see them
//! and audits every read.

use quality::source_records::{RecordsAsk, RecordsPage, PAGE};

use crate::extract::source_table;
use crate::{invalid, quoted, Error, Runner};

impl Runner {
    /// The page `ask` names of the attached source's table, for `source`.
    /// An order that is not one of the table's columns is refused.
    pub fn source_page(&self, source: &str, ask: &RecordsAsk) -> Result<RecordsPage, Error> {
        ask.check().map_err(invalid)?;
        let table = source_table(&ask.table)?;
        let columns: Vec<String> = self
            .source_columns(&ask.table)?
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        let order = match &ask.order {
            Some(c) if !columns.contains(c) => return Err(invalid("the order is no column")),
            Some(c) => format!(
                " ORDER BY {} {}",
                quoted(c)?,
                if ask.descending { "DESC" } else { "ASC" }
            ),
            None => String::new(),
        };
        let cast = columns
            .iter()
            .map(|c| Ok(format!("CAST({} AS VARCHAR)", quoted(c)?)))
            .collect::<Result<Vec<_>, Error>>()?;
        let sql = format!(
            "SELECT {} FROM {table}{order} LIMIT {} OFFSET {}",
            cast.join(", "),
            PAGE + 1,
            ask.offset()
        );
        let mut stmt = self.con.prepare(&sql)?;
        let width = columns.len();
        let rows = stmt
            .query_map([], |r| {
                (0..width)
                    .map(|i| r.get::<_, Option<String>>(i))
                    .collect::<Result<Vec<_>, _>>()
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RecordsPage::new(source, &ask.table, columns, rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runner() -> Runner {
        let runner = Runner::new().unwrap();
        runner
            .con
            .execute_batch(
                "ATTACH ':memory:' AS src;
                 CREATE SCHEMA src.loans;
                 CREATE TABLE src.loans.credits AS
                   SELECT i AS id, 'c' || i AS code, CASE WHEN i % 7 = 0 THEN NULL ELSE i * 2 END AS amount
                   FROM range(1, 1503) t(i);",
            )
            .unwrap();
        runner
    }

    fn ask(order: Option<&str>, descending: bool, page: u32) -> RecordsAsk {
        RecordsAsk {
            table: "loans.credits".into(),
            order: order.map(str::to_string),
            descending,
            page,
        }
    }

    #[test]
    fn the_first_and_the_last_records_are_a_page_and_say_whether_there_are_more() {
        let r = runner();
        let first = r.source_page("core", &ask(Some("id"), false, 0)).unwrap();
        assert_eq!(first.columns, ["id", "code", "amount"]);
        assert_eq!(first.rows.len(), PAGE as usize);
        assert!(first.more);
        assert_eq!(
            first.rows[0],
            [Some("1".into()), Some("c1".into()), Some("2".into())]
        );
        assert_eq!(first.rows[6][2], None, "empty stays empty");

        let last = r.source_page("core", &ask(Some("id"), true, 0)).unwrap();
        assert_eq!(last.rows[0][0].as_deref(), Some("1502"));
        let second = r.source_page("core", &ask(Some("id"), false, 1)).unwrap();
        assert_eq!((second.rows.len(), second.more), (502, false));
        assert_eq!(second.rows[0][0].as_deref(), Some("1001"));
    }

    #[test]
    fn an_order_that_is_no_column_or_a_last_page_without_one_is_refused() {
        let r = runner();
        assert!(r.source_page("core", &ask(Some("nope"), false, 0)).is_err());
        assert!(r.source_page("core", &ask(None, true, 0)).is_err());
        assert!(r.source_page("core", &ask(None, false, 0)).is_ok());
    }
}
