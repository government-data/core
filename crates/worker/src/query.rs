//! The `query` job (decision 0054): a console statement over the
//! lake, attached with the job's read-only credentials and locked down, run
//! wrapped as one query of at most 200 rows. Its rows, or the engine's
//! message when it fails, go to the API only, which masks and audits them;
//! nothing but counts is logged here.

use quality::console::{wrapped, ConsoleResult, MAX_ROWS};
use quality::worker_api::JobDetails;
use quality_rules_duckdb::{LakeConnection, Runner};

use crate::evaluate::{engine, failed, Failed};

/// Runs the job's statement; `ca_file` is a CA to trust for the lake.
pub(crate) fn query(job: &JobDetails, ca_file: Option<&str>) -> Result<ConsoleResult, Failed> {
    let sql = job
        .parameters
        .get("sql")
        .and_then(|s| s.as_str())
        .ok_or_else(|| failed("query: the job has no statement"))?;
    let statement = wrapped(sql).map_err(|_| failed("query: the statement is not one query"))?;
    let lake = job
        .lake
        .as_ref()
        .ok_or_else(|| failed("query: no lake access came with the job"))?;
    let mut runner = Runner::new().map_err(engine("starting"))?;
    runner
        .attach_lake(&LakeConnection {
            endpoint: &lake.endpoint,
            warehouse: &lake.warehouse,
            access_key: &lake.access_key,
            secret_key: &lake.secret_key,
            session_token: &lake.session_token,
            ca_file,
        })
        .map_err(engine("attaching the lake"))?;
    // The person's statement may fail: that is its result, said to them.
    Ok(match runner.console(&statement) {
        Ok((columns, mut rows)) => {
            let more = rows.len() > MAX_ROWS;
            rows.truncate(MAX_ROWS);
            ConsoleResult {
                columns,
                rows,
                more,
                error: None,
            }
        }
        Err(e) => ConsoleResult {
            columns: Vec::new(),
            rows: Vec::new(),
            more: false,
            error: Some(e.to_string()),
        },
    })
}
