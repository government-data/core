//! The `records` job (decision 0049, points 3 and 4): a page of a source
//! table's records, read through DuckDB with the source's credentials,
//! locked down. Values go to the API only, which masks and audits
//! them; nothing but counts is logged. An engine's message may quote a
//! value, so only the stage it failed in is kept.

use quality::source_records::{RecordsAsk, RecordsPage};
use quality::worker_api::JobDetails;

use crate::evaluate::{failed, Failed};
use crate::extract::with_source;

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
