//! A dataset's tables in the lake (decisions 0017, 0037 and 0061): its
//! bronze table, as extracted, and its silver table, typed and normalized
//! from it; each in a namespace of the dataset's, named after the source's
//! extract the mapping reads.

use crate::mapping::Mapping;

/// Silver's column with the entity's key normalized.
pub const ENTITY_KEY: &str = "_entity_key";
/// Silver's column saying whether the entity's key normalized.
pub const KEY_NORMALIZED: &str = "_entity_key_normalized";
/// Silver's column with the digest of what derived the row.
pub const DERIVED_BY: &str = "_mapping";

/// The bronze table an extraction writes: `<dataset>_bronze.<source>`.
pub fn bronze(mapping: &Mapping) -> (String, String) {
    (
        format!("{}_bronze", mapping.dataset.value),
        mapping.source.value.clone(),
    )
}

/// The silver table derived from it, `<dataset>_silver.<source>`; none
/// when a field's name would take one of silver's own columns (`cut`, or
/// any starting with `_`), so the dataset is read from bronze (decision
/// 0061, point 2).
pub fn silver(mapping: &Mapping) -> Option<(String, String)> {
    let clash = mapping
        .fields
        .keys()
        .any(|f| f == "cut" || f.starts_with('_'));
    (!clash).then(|| {
        (
            format!("{}_silver", mapping.dataset.value),
            mapping.source.value.clone(),
        )
    })
}

/// A dataset's namespaces: bronze's and silver's.
pub fn namespaces(dataset: &str) -> [String; 2] {
    [format!("{dataset}_bronze"), format!("{dataset}_silver")]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(field: &str) -> Mapping {
        Mapping::parse(&format!(
            "dataset: loans\nsource: loans_2026\nfields:\n  {field}: {{ column: c1, type: text }}\n"
        ))
        .unwrap()
    }

    #[test]
    fn a_dataset_has_silver_unless_a_field_takes_its_columns() {
        let m = mapping("Documento");
        assert_eq!(bronze(&m), ("loans_bronze".into(), "loans_2026".into()));
        assert_eq!(
            silver(&m),
            Some(("loans_silver".into(), "loans_2026".into()))
        );
        assert_eq!(silver(&mapping("cut")), None);
        assert_eq!(silver(&mapping("_mapping")), None);
        assert_eq!(namespaces("loans"), ["loans_bronze", "loans_silver"]);
    }
}
