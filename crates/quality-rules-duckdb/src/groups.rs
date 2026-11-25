//! Each group of a cut (decision 0031's `group_metrics`): per field holding
//! the `dimension` role and each of its values, its records, those with a
//! serious failure and their mean ICD, over the records' ICD (`__icd`).
//! The largest groups of each dimension only: a dimension has few values.

use quality::mapping::{FieldRole, Mapping, Priority};
use quality::worker_api::GroupMetrics;
use quality_rules_model::Severity;

use crate::icd::IcdRule;
use crate::{quoted, Error, Runner};

/// The most groups kept per dimension, the largest.
const MOST_GROUPS: usize = 1000;

/// Whether a record has a serious failure: a blocking or major rule that
/// reads a critical or high field fails (decision 0031's `group_metrics`,
/// until rules carry their cause).
pub(crate) fn serious(mapping: &Mapping, rules: &[IcdRule]) -> String {
    let weighty = |name: &String| {
        mapping
            .fields
            .get(name)
            .is_some_and(|f| matches!(f.value.priority, Priority::Critical | Priority::High))
    };
    let failing: Vec<String> = rules
        .iter()
        .enumerate()
        .filter(|(_, r)| matches!(r.severity, Severity::Blocking | Severity::Major))
        .filter(|(_, r)| r.reads.iter().any(weighty))
        .map(|(i, _)| format!("COALESCE(\"r{i}\", FALSE)"))
        .collect();
    if failing.is_empty() {
        "FALSE".to_string()
    } else {
        failing.join(" OR ")
    }
}

/// The fields holding the `dimension` role, quoted.
pub(crate) fn dimensions(mapping: &Mapping) -> Result<Vec<String>, Error> {
    mapping
        .fields
        .iter()
        .filter(|(_, f)| f.value.roles.contains(&FieldRole::Dimension))
        .map(|(name, _)| quoted(name))
        .collect()
}

impl Runner {
    /// The groups of the `n` dimensions in `__icd`, in the mapping's order.
    pub(crate) fn groups(&self, mapping: &Mapping, n: usize) -> Result<Vec<GroupMetrics>, Error> {
        let names = mapping
            .fields
            .iter()
            .filter(|(_, f)| f.value.roles.contains(&FieldRole::Dimension))
            .map(|(name, _)| name);
        let mut out = Vec::new();
        for (j, name) in names.enumerate().take(n) {
            let sql = format!(
                "SELECT \"__g{j}\" AS g, count(*) AS n, count(*) FILTER (WHERE \"__serious\"),
                        avg(\"__icd\")
                   FROM \"__icd\" GROUP BY \"__g{j}\"
                  ORDER BY n DESC NULLS LAST, g ASC NULLS LAST LIMIT {MOST_GROUPS}"
            );
            let mut stmt = self.con.prepare(&sql)?;
            let rows = stmt.query_map([], |r| {
                Ok(GroupMetrics {
                    dimension: name.clone(),
                    group: r.get(0)?,
                    records: r.get(1)?,
                    serious: r.get(2)?,
                    icd: r.get(3)?,
                })
            })?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }
}
