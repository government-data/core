//! The `records` job (decision 0049, points 3 and 4): a page of a source
//! table's records, read through the source's engine: DuckDB with the
//! source's credentials, locked down, or Trino through the source's
//! catalog, with none. Values go to the API only, which masks and audits
//! them; nothing but counts is logged. An engine's message may quote a
//! value, so only the stage it failed in is kept.

use quality::source_records::{trino_columns, trino_statement, RecordsAsk, RecordsPage};
use quality::worker_api::JobDetails;
use serde_json::Value;

use crate::evaluate::{failed, Failed};
use crate::extract::with_source;
use crate::trino_job::JobTrino;

/// What the job asks.
fn asked(job: &JobDetails) -> Result<(String, RecordsAsk), Failed> {
    let source = job
        .source
        .as_ref()
        .ok_or_else(|| failed("records: the job came with no source"))?;
    let ask: RecordsAsk = serde_json::from_value(job.parameters.clone())
        .map_err(|_| failed("records: the job does not say what to read"))?;
    ask.check()
        .map_err(|e| failed(format!("records: the job cannot be read ({e:?})")))?;
    Ok((source.name.clone(), ask))
}

/// Reads the page through DuckDB, the source attached read-only.
pub(crate) fn read(job: &JobDetails) -> Result<RecordsPage, Failed> {
    let (source, ask) = asked(job)?;
    let mut runner = with_source(job, "records")?;
    runner
        .lock_down()
        .map_err(|_| failed("records: the engine failed while locking down"))?;
    runner
        .source_page(&source, &ask)
        .map_err(|_| failed("records: the engine failed while reading"))
}

/// A value as Trino gives it, as text; `None` where empty.
fn as_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

/// Reads the page through the source's catalog on Trino.
pub(crate) async fn read_on_trino(
    job: &JobDetails,
    trino: &JobTrino<'_>,
) -> Result<RecordsPage, Failed> {
    let (source, ask) = asked(job)?;
    let engine =
        |stage: &'static str| move |_| failed(format!("records: the engine failed while {stage}"));
    let names = trino_columns(&source, &ask).map_err(|_| failed("records: no such table"))?;
    let columns: Vec<String> = trino
        .execute(&names)
        .await
        .map_err(engine("reading the columns"))?
        .iter()
        .filter_map(|r| r.first().and_then(Value::as_str).map(str::to_string))
        .collect();
    let page = trino_statement(&source, &ask).map_err(|_| failed("records: no such table"))?;
    let rows = trino
        .execute(&page)
        .await
        .map_err(engine("reading"))?
        .iter()
        .map(|r| r.iter().map(as_text).collect())
        .collect();
    Ok(RecordsPage::new(&source, &ask.table, columns, rows))
}
