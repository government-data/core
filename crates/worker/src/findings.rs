//! An evaluation's remediation queues (decision 0059): its findings, each
//! record by the keyed hash of its key with the installation's entity key, and
//! each queue's open findings by the cut they opened at. Only the counts
//! are reported; without the key (an older application) there are none.

use quality::entity_key::EntityKey;
use quality::mapping::Mapping;
use quality::worker_api::{QueueFindings, SfcCell};
use quality_rules_duckdb::{IcdRule, Runner};

use crate::evaluate::{engine, Failed};

/// The queues of the cut whose ICD the runner just computed, a finding's
/// closeness from its field's SFC `cells` as the ICD counted them. Until the
/// findings are kept in the lake, no previous cut is read: every finding
/// opens at this cut.
pub(crate) fn queues(
    runner: &Runner,
    mapping: &Mapping,
    rules: &[IcdRule],
    cut: &str,
    cells: &[SfcCell],
    key: Option<&EntityKey>,
) -> Result<Vec<QueueFindings>, Failed> {
    let Some(key) = key else {
        return Ok(Vec::new());
    };
    runner
        .findings(mapping, rules, cut, &key.0, cells, None)
        .map_err(engine("finding the queues"))
}
