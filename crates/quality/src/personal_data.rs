//! Who sees personal data whole (decision 0064): `quality:ViewPersonalData`
//! is checked on the dataset, `pipeline/<dataset>`, everywhere. The records
//! of a source's table need it on every dataset that reads the table; the
//! console, and a table no dataset reads, on every dataset.

use contracts::Caller;

use crate::catalog::{Catalog, DatasetMapping};
use crate::mapping::Mapping;
use crate::rule_test::PERSONAL;

/// Whether `caller` sees personal data of every dataset, those to come
/// included: allowed on all, denied on none.
pub fn on_every_dataset(caller: &Caller) -> bool {
    caller.can_every(PERSONAL, "pipeline")
}

/// The datasets whose mapping on `main` reads `source`'s `table`; `None`
/// if a mapping does not parse, so it cannot say. A table is matched by
/// its last part, case ignored: a mapping that may read it counts.
pub fn readers(mappings: &[DatasetMapping], source: &str, table: &str) -> Option<Vec<String>> {
    let name = |t: &str| t.rsplit('.').next().unwrap_or(t).to_lowercase();
    let mut datasets = Vec::new();
    for m in mappings {
        let from = Mapping::parse(&m.yaml).ok()?.from;
        if from.is_some_and(|f| f.value.source == source && name(&f.value.table) == name(table)) {
            datasets.push(m.dataset.clone());
        }
    }
    Some(datasets)
}

/// Whether `caller` sees the values of `source`'s `table`: with the
/// permission on every dataset that reads it, or on every dataset when
/// none does or the catalog cannot say.
pub async fn sees_table(
    caller: &Caller,
    catalog: Option<&dyn Catalog>,
    source: &str,
    table: &str,
) -> bool {
    let mappings = match catalog {
        Some(catalog) => catalog.mappings().await.map_err(|e| {
            tracing::error!("personal data: catalog: {e}");
        }),
        _ => Err(()),
    };
    match mappings.ok().and_then(|m| readers(&m, source, table)) {
        Some(datasets) if !datasets.is_empty() => datasets
            .iter()
            .all(|d| caller.can(PERSONAL, &format!("pipeline/{d}"))),
        _ => on_every_dataset(caller),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(dataset: &str, from: &str) -> DatasetMapping {
        let yaml = format!(
            "dataset: {dataset}\nsource: {dataset}\n{from}key: [id]\nfields:\n  id: {{ column: ID, type: text }}\n"
        );
        DatasetMapping {
            dataset: dataset.into(),
            yaml,
        }
    }

    #[test]
    fn the_datasets_that_read_a_table() {
        let mappings = [
            mapping("credits", "from: { source: core, table: dbo.clients }\n"),
            mapping("savings", "from: { source: core, table: CLIENTS }\n"),
            mapping("loans", "from: { source: core, table: dbo.loans }\n"),
            mapping("other", "from: { source: branch, table: dbo.clients }\n"),
            mapping("unread", ""),
        ];
        assert_eq!(
            readers(&mappings, "core", "dbo.clients"),
            Some(vec!["credits".into(), "savings".into()])
        );
        assert_eq!(readers(&mappings, "core", "dbo.accounts"), Some(vec![]));
        let broken = [DatasetMapping {
            dataset: "x".into(),
            yaml: "not: [a mapping".into(),
        }];
        assert_eq!(readers(&broken, "core", "dbo.clients"), None);
    }
}
