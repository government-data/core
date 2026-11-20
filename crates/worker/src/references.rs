//! The other datasets a dataset's rules look up (decision 0056), loaded
//! for an evaluation or a rule's test at the same cut.

use std::collections::BTreeSet;

use quality::catalog::CatalogSnapshot;
use quality::worker_api::SnapshotRead;
use quality_rules_duckdb::{LakeRead, Runner, SilverAt};

use crate::evaluate::{engine, failed, parse_mapping, Failed};

/// What a run's manifest records of a dataset's cut as it was read: its
/// snapshot, and `silver` if from silver (decision 0061).
pub(crate) fn snapshot_read(dataset: &str, read: LakeRead) -> SnapshotRead {
    SnapshotRead {
        dataset: dataset.to_string(),
        snapshot: read.snapshot,
        layer: read.silver.then(|| "silver".to_string()),
    }
}

/// The other datasets the rules look up, each through its mapping, at the
/// same cut (the only `as_of` there is), from silver's current snapshot
/// when it holds the cut; the snapshots read. One with no
/// records at the cut stops the evaluation (decision 0056).
pub(crate) fn load_references(
    runner: &mut Runner,
    catalog: &CatalogSnapshot,
    cut: &str,
) -> Result<Vec<SnapshotRead>, Failed> {
    let mut wanted = BTreeSet::new();
    for source in &catalog.rules {
        if let Ok(file) = quality_rules::parse(&source.yaml) {
            wanted.extend(file.references.values().map(|r| r.dataset.value.clone()));
        }
    }
    let mut read = Vec::new();
    for dataset in wanted {
        let other = catalog
            .others
            .iter()
            .find(|m| m.dataset == dataset)
            .ok_or_else(|| {
                failed(format!(
                    "evaluate: the catalog has no mapping for {dataset}"
                ))
            })?;
        let mapping = parse_mapping(&dataset, &other.yaml)?;
        let loaded = runner
            .load_mapped_lake(&mapping, cut, None, Some(SilverAt::Current))
            .map_err(engine(&format!("reading {dataset}")))?;
        let records = runner
            .records(&dataset)
            .map_err(engine(&format!("counting {dataset}")))?;
        present(&dataset, cut, records)?;
        read.push(snapshot_read(&dataset, loaded));
    }
    Ok(read)
}

/// Every lookup in a reference with no records fails: that says nothing
/// about the records, so the evaluation stops, naming it.
fn present(reference: &str, cut: &str, records: u64) -> Result<(), Failed> {
    if records == 0 {
        return Err(failed(format!(
            "evaluate: the reference {reference} has no records at the cut {cut}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_with_no_records_at_the_cut_stops_the_evaluation() {
        assert_eq!(
            present("advisors", "2026-03-31", 0).unwrap_err().0,
            "evaluate: the reference advisors has no records at the cut 2026-03-31"
        );
        assert!(present("advisors", "2026-03-31", 1).is_ok());
    }
}
