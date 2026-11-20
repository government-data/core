//! What an evaluation measures of its fields' distributions (decision
//! 0031), by the roles of the mapping: the update date's records by age,
//! each drift variable's percentiles, and the outlier amount's outliers
//! within each dimension's groups.

use quality::mapping::{FieldRole, Mapping};
use quality::worker_api::{Distribution, FieldQuality, OutlierGroup};
use quality_rules_duckdb::Runner;

use crate::evaluate::{engine, Failed};

fn holding(mapping: &Mapping, role: FieldRole) -> Vec<&str> {
    mapping
        .fields
        .iter()
        .filter(|(_, f)| f.value.roles.contains(&role))
        .map(|(name, _)| name.as_str())
        .collect()
}

/// The records by the age of the field holding `updated`, on its profile.
pub(crate) fn recency(
    runner: &Runner,
    dataset: &str,
    mapping: &Mapping,
    cut: &str,
    fields: &mut [FieldQuality],
) -> Result<(), Failed> {
    let Some(name) = holding(mapping, FieldRole::Updated).first().copied() else {
        return Ok(());
    };
    let counts = runner
        .recency(dataset, name, cut)
        .map_err(engine("measuring the update recency"))?;
    if let Some(f) = fields.iter_mut().find(|f| f.field == name) {
        f.recency = Some(counts);
    }
    Ok(())
}

/// Each drift variable's percentiles; none for one without values.
pub(crate) fn distributions(
    runner: &Runner,
    dataset: &str,
    mapping: &Mapping,
) -> Result<Vec<Distribution>, Failed> {
    let mut out = Vec::new();
    for variable in holding(mapping, FieldRole::Drift) {
        let percentiles = runner
            .percentiles(dataset, variable)
            .map_err(engine("measuring the drift variables"))?;
        out.extend(percentiles.map(|percentiles| Distribution {
            variable: variable.to_string(),
            percentiles,
        }));
    }
    Ok(out)
}

/// The outlier amount's outliers within each dimension's groups; none
/// without both.
pub(crate) fn outliers(
    runner: &Runner,
    dataset: &str,
    mapping: &Mapping,
) -> Result<Vec<OutlierGroup>, Failed> {
    let Some(amount) = holding(mapping, FieldRole::OutlierAmount).first().copied() else {
        return Ok(Vec::new());
    };
    runner
        .outliers(dataset, amount, &holding(mapping, FieldRole::Dimension))
        .map_err(engine("seeking the outliers"))
}
