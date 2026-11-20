//! An extraction on Trino (decision 0048, point 3): the worker's statements,
//! pure, as verified on Trino (`tools/verify/trino_sqlserver.sh`); and the
//! job's details over an ephemeral PostgreSQL: its source's name and table
//! but no credential, its namespace made, no lake credential given and
//! nothing recorded. Synthetic names only.

pub mod worker;

use axum::http::StatusCode;
use quality::cuts::SourceColumn;
use quality::source::Source;
use quality::source_store;
use quality::trino_extract::{number, source_columns, text, ExtractError, Extraction};
use quality::worker_api::JobDetails;
use serde_json::json;
use worker::*;

fn ledger() -> Extraction {
    Extraction::new("ledger", "dbo.Clients", "loans", "clients", "2026-11-03").unwrap()
}

#[test]
fn a_first_extraction_creates_its_table_from_the_sources_with_the_cut() {
    let e = ledger();
    assert_eq!(
        e.columns(),
        "SELECT column_name, data_type FROM src_ledger.information_schema.columns \
         WHERE table_schema = 'dbo' AND table_name = 'clients' ORDER BY ordinal_position"
    );
    assert_eq!(
        e.exists(),
        "SELECT count(*) FROM lake.information_schema.tables \
         WHERE table_schema = 'loans_bronze' AND table_name = 'clients'"
    );
    assert_eq!(
        e.create(),
        "CREATE TABLE lake.\"loans_bronze\".\"clients\" WITH (partitioning = ARRAY['cut']) AS \
         SELECT s.*, DATE '2026-11-03' AS cut FROM src_ledger.\"dbo\".\"Clients\" s"
    );
}

#[test]
fn a_later_extraction_replaces_its_cut_column_by_column() {
    let e = ledger();
    let bronze = "lake.\"loans_bronze\".\"clients\"";
    assert_eq!(
        e.cut_rows(),
        format!("SELECT count(*) FROM {bronze} WHERE cut = DATE '2026-11-03'")
    );
    assert_eq!(
        e.delete(),
        format!("DELETE FROM {bronze} WHERE cut = DATE '2026-11-03'")
    );
    let columns = [("id", "integer"), ("odd\"name", "varchar(10)")].map(|(n, t)| SourceColumn {
        name: n.into(),
        data_type: t.into(),
    });
    assert_eq!(
        e.insert(&columns),
        format!(
            "INSERT INTO {bronze} (\"id\", \"odd\"\"name\", cut) SELECT \"id\", \"odd\"\"name\", \
             DATE '2026-11-03' FROM src_ledger.\"dbo\".\"Clients\""
        )
    );
    assert_eq!(
        e.snapshot(),
        "SELECT CAST(snapshot_id AS varchar) FROM lake.\"loans_bronze\".\"clients$snapshots\" \
         ORDER BY committed_at DESC LIMIT 1"
    );
}

#[test]
fn what_cannot_be_written_is_said_and_trinos_answers_are_read() {
    let bad_cut = Extraction::new("ledger", "dbo.clients", "loans", "clients", "2026-11-3");
    assert_eq!(bad_cut.unwrap_err(), ExtractError::Cut);
    let injected = Extraction::new("ledger", "dbo.c", "loans", "c", "2026-11-03' OR 1=1");
    assert_eq!(injected.unwrap_err(), ExtractError::Cut);
    let no_schema = Extraction::new("ledger", "clients", "loans", "clients", "2026-11-03");
    assert_eq!(no_schema.unwrap_err(), ExtractError::Table);

    let columns = source_columns(&[vec![json!("id"), json!("integer")], vec![json!(1)]]);
    assert_eq!(columns.len(), 1);
    assert_eq!(number(&[vec![json!(3)]]), Some(3));
    assert_eq!(text(&[vec![json!("8812")]]).as_deref(), Some("8812"));
    assert_eq!(number(&[]), None);
}

#[tokio::test]
async fn an_extraction_on_trino_gets_no_credential_and_its_namespace_is_made() {
    let f = fixture().await;
    tenancy::within(tenant(B), async {
        let source = Source {
            name: "core".into(),
            driver: "sqlserver".into(),
            fields: [("host", "db.example"), ("database", "core")]
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .into(),
            secrets: [("user", "reader"), ("password", "synthetic")]
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .into(),
        };
        source_store::put(&f.app, &Mirror, &source).await.unwrap();
    })
    .await;
    let extract = f.cut(B).await;
    let uri = format!("/internal/jobs/{B}/{extract}");
    let (status, body) = f.call("GET", &uri, "token-w1", None).await;
    assert_eq!(status, StatusCode::OK);
    let details: JobDetails = serde_json::from_slice(&body).unwrap();
    let source = details.source.unwrap();
    assert_eq!(
        (source.name.as_str(), source.table.as_str()),
        ("core", "public.clients")
    );
    assert!(source.on_trino() && source.secrets.is_empty() && source.fields.is_empty());
    assert!(details.lake.is_none(), "Trino writes with its own access");
    assert_eq!(*f.tables.1.lock().unwrap(), ["clientes_bronze"]);
    // No credential of the source or the lake: its token for Trino alone,
    // recorded as given (decision 0062).
    let given: Vec<String> = {
        f.issued
            .given
            .lock()
            .unwrap()
            .iter()
            .map(|g| g.2.clone())
            .collect()
    };
    assert_eq!(given.len(), 1, "{given:?}");
    assert!(given[0].starts_with("trino/src_core"), "{given:?}");
}
