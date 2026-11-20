//! The `lake_tables` job (decision 0043, point 1): the tenant's lake,
//! attached with credentials that read the tables' metadata and no data
//! file, its namespaces as schemas and each table with its columns, never
//! a row. Only the stage it failed in is kept: the platform reads it as a
//! reason (`quality::source_tables::reason`).

use quality::lake_tables::LakeListing;
use quality::source_tables::MAX_TABLES;
use quality::worker_api::JobDetails;
use quality_rules_duckdb::{LakeConnection, Runner};

use crate::evaluate::{failed, Failed};

/// Lists the job's lake; `ca_file` is a CA to trust for it besides the
/// system's.
pub(crate) fn list(job: &JobDetails, ca_file: Option<&str>) -> Result<LakeListing, Failed> {
    let lake = job
        .lake
        .as_ref()
        .ok_or_else(|| failed("lake_tables: no lake access came with the job"))?;
    let mut runner =
        Runner::new().map_err(|_| failed("lake_tables: the engine failed while starting"))?;
    runner
        .attach_lake(&LakeConnection {
            endpoint: &lake.endpoint,
            warehouse: &lake.warehouse,
            access_key: &lake.access_key,
            secret_key: &lake.secret_key,
            session_token: &lake.session_token,
            ca_file,
        })
        .map_err(|_| failed("lake_tables: the engine failed while attaching the lake"))?;
    let (tables, truncated) = runner
        .list_lake(MAX_TABLES)
        .map_err(|_| failed("lake_tables: the engine failed while listing"))?;
    Ok(LakeListing { tables, truncated })
}
