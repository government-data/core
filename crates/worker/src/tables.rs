//! The `tables` job (decision 0042): the source's tables, read-only
//! through its driver with the connection locked down, names and columns
//! only, the system's schemas left out. An engine's message may quote a
//! name, so only the stage it failed in is kept: the platform reads it as
//! a reason (`quality::source_tables::reason`).

use quality::source_tables::{system_schemas, TableListing, MAX_TABLES};
use quality::worker_api::JobDetails;

use crate::evaluate::{failed, Failed};
use crate::extract::with_source;

/// Lists the job's source.
pub(crate) fn list(job: &JobDetails) -> Result<TableListing, Failed> {
    let source = job
        .source
        .as_ref()
        .ok_or_else(|| failed("tables: the job came with no source"))?;
    let mut runner = with_source(job, "tables")?;
    runner
        .lock_down()
        .map_err(|_| failed("tables: the engine failed while locking down"))?;
    let (tables, truncated) = runner
        .list_tables(system_schemas(&source.driver), MAX_TABLES)
        .map_err(|_| failed("tables: the engine failed while listing"))?;
    Ok(TableListing {
        source: source.name.clone(),
        tables,
        truncated,
    })
}
