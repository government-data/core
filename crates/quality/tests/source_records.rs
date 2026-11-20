//! A source table's records as asked and reported (decision 0049, point 3),
//! pure: what can be asked, the page a job reports and how it is masked,
//! and the statement Trino runs. Synthetic values only.

use quality::source_records::{
    trino_columns, trino_statement, AskError, RecordsAsk, RecordsPage, PAGE,
};

fn ask(table: &str, order: Option<&str>, descending: bool, page: u32) -> RecordsAsk {
    RecordsAsk {
        table: table.into(),
        order: order.map(str::to_string),
        descending,
        page,
    }
}

#[test]
fn trino_reads_a_page_and_one_more_in_the_order_asked() {
    assert_eq!(
        trino_statement("ledger", &ask("dbo.clients", None, false, 0)).unwrap(),
        format!(
            "SELECT * FROM src_ledger.\"dbo\".\"clients\" OFFSET 0 LIMIT {}",
            PAGE + 1
        )
    );
    assert_eq!(
        trino_statement("ledger", &ask("dbo.clients", Some("odd\"col"), true, 2)).unwrap(),
        format!(
            "SELECT * FROM src_ledger.\"dbo\".\"clients\" ORDER BY \"odd\"\"col\" DESC \
             OFFSET {} LIMIT {}",
            2 * PAGE,
            PAGE + 1
        )
    );
}

#[test]
fn trino_names_the_tables_columns_in_their_order() {
    assert_eq!(
        trino_columns("ledger", &ask("dbo.Clients", None, false, 0)).unwrap(),
        "SELECT column_name FROM src_ledger.information_schema.columns \
         WHERE table_schema = 'dbo' AND table_name = 'clients' ORDER BY ordinal_position"
    );
}

#[test]
fn what_cannot_be_asked_is_said() {
    let no_schema = trino_statement("ledger", &ask("clients", None, false, 0));
    assert_eq!(no_schema.unwrap_err(), AskError::Table);
    let three = ask("a.b.c", None, false, 0).check();
    assert_eq!(three.unwrap_err(), AskError::Table);
    assert_eq!(
        ask("dbo.t", None, true, 0).check().unwrap_err(),
        AskError::NoOrder
    );
    assert_eq!(
        ask("dbo.t", None, false, 1000).check().unwrap_err(),
        AskError::TooFar
    );
}

#[test]
fn a_page_says_whether_there_are_more_and_masks_every_value() {
    let rows: Vec<_> = (0..=PAGE)
        .map(|i| vec![Some(format!("10000{i}")), None])
        .collect();
    let page = RecordsPage::new(
        "ledger",
        "dbo.clients",
        vec!["doc".into(), "note".into()],
        rows,
    );
    assert_eq!((page.rows.len(), page.more), (PAGE as usize, true));
    assert!(page.is_valid());
    let masked = page.masked();
    assert_eq!(masked.rows[1], [Some("••••0001".to_string()), None]);
    assert_eq!(masked.columns, ["doc", "note"], "names are not values");

    let short = RecordsPage::new("ledger", "dbo.t", vec!["a".into()], vec![vec![None]]);
    assert!(!short.more);
    let wrong = RecordsPage::new("ledger", "dbo.t", vec!["a".into()], vec![vec![None, None]]);
    assert!(!wrong.is_valid());
}
