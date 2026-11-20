//! The chart catalog (decision 0031): each chart a parameterized query over
//! the results model, with what it needs from a dataset. A query names its
//! parameters (`:dataset`, `:cut`, `:previous_cut`); they are bound, never
//! spliced into the text. So far the catalog is the module's own, built
//! in; the ICD's (decision 0057) need an entity, and a segment; the
//! update recency an update date; the groups' a dimension; the outliers
//! an outlier amount and a dimension; the PSI a drift variable; the SFC
//! view (decision 0058) a field holding the `sfc` role; the remediation
//! queues (decision 0059) a dimension; the time per stage, from the run
//! log, nothing.

use serde::Serialize;
use utoipa::ToSchema;

/// A chart of the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct Chart {
    /// Its id, the key of its texts in the web.
    pub id: &'static str,
    /// How the web draws it: `bar`, `line`, `scatter` or `table`.
    pub kind: &'static str,
    /// The field roles it needs from a dataset ([`crate::mapping::FieldRole`]
    /// by name).
    pub needs: &'static [&'static str],
    #[serde(skip)]
    pub sql: &'static str,
}

/// Every chart, in the order a dashboard shows them.
pub const CATALOG: &[Chart] = &[
    Chart {
        id: "trend_by_cut",
        kind: "line",
        needs: &[],
        sql: include_str!("../charts/trend_by_cut.sql"),
    },
    Chart {
        id: "failures_by_dimension",
        kind: "bar",
        needs: &[],
        sql: include_str!("../charts/failures_by_dimension.sql"),
    },
    Chart {
        id: "failures_by_field",
        kind: "bar",
        needs: &[],
        sql: include_str!("../charts/failures_by_field.sql"),
    },
    Chart {
        id: "lowest_icd",
        kind: "bar",
        needs: &[],
        sql: include_str!("../charts/lowest_icd.sql"),
    },
    Chart {
        id: "icd_distribution",
        kind: "bar",
        needs: &["entity"],
        sql: include_str!("../charts/icd_distribution.sql"),
    },
    Chart {
        id: "bands_by_segment",
        kind: "table",
        needs: &["entity", "segment"],
        sql: include_str!("../charts/bands_by_segment.sql"),
    },
    Chart {
        id: "band_transitions",
        kind: "table",
        needs: &["entity"],
        sql: include_str!("../charts/band_transitions.sql"),
    },
    Chart {
        id: "sfc_view",
        kind: "scatter",
        needs: &["sfc"],
        sql: include_str!("../charts/sfc_view.sql"),
    },
    Chart {
        id: "groups_p_chart",
        kind: "scatter",
        needs: &["dimension"],
        sql: include_str!("../charts/groups_p_chart.sql"),
    },
    Chart {
        id: "largest_groups",
        kind: "bar",
        needs: &["dimension"],
        sql: include_str!("../charts/largest_groups.sql"),
    },
    Chart {
        id: "groups_changed",
        kind: "bar",
        needs: &["dimension"],
        sql: include_str!("../charts/groups_changed.sql"),
    },
    Chart {
        id: "outliers_by_group",
        kind: "bar",
        needs: &["outlier_amount", "dimension"],
        sql: include_str!("../charts/outliers_by_group.sql"),
    },
    Chart {
        id: "psi_by_decile",
        kind: "bar",
        needs: &["drift"],
        sql: include_str!("../charts/psi_by_decile.sql"),
    },
    Chart {
        id: "remediation_queues",
        kind: "bar",
        needs: &["dimension"],
        sql: include_str!("../charts/remediation_queues.sql"),
    },
    Chart {
        id: "update_recency",
        kind: "bar",
        needs: &["updated"],
        sql: include_str!("../charts/update_recency.sql"),
    },
    Chart {
        id: "profiling",
        kind: "bar",
        needs: &[],
        sql: include_str!("../charts/profiling.sql"),
    },
    Chart {
        id: "conformity_by_rule",
        kind: "table",
        needs: &[],
        sql: include_str!("../charts/conformity_by_rule.sql"),
    },
    Chart {
        id: "time_by_stage",
        kind: "bar",
        needs: &[],
        sql: include_str!("../charts/time_by_stage.sql"),
    },
];

/// The charts of `catalog` a dataset whose fields hold `roles` (by name)
/// can show.
pub fn meeting<'a>(
    catalog: &'static [Chart],
    roles: &'a [&str],
) -> impl Iterator<Item = &'static Chart> + 'a {
    catalog
        .iter()
        .filter(|c| c.needs.iter().all(|n| roles.contains(n)))
}

/// The chart by id.
pub fn chart(id: &str) -> Option<&'static Chart> {
    CATALOG.iter().find(|c| c.id == id)
}

/// The parameters a chart is run with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameters<'a> {
    pub dataset: &'a str,
    /// `YYYY-MM-DD`.
    pub cut: &'a str,
    /// The evaluated cut before it, if any.
    pub previous_cut: Option<&'a str>,
}

impl Parameters<'_> {
    fn value(&self, name: &str) -> Result<Option<String>, String> {
        match name {
            "dataset" => Ok(Some(self.dataset.to_string())),
            "cut" => Ok(Some(self.cut.to_string())),
            "previous_cut" => Ok(self.previous_cut.map(str::to_string)),
            other => Err(other.to_string()),
        }
    }
}

/// The query with its named parameters as `$1`, `$2`…, and the values to
/// bind, in order; a parameter used twice is bound once. A `::` cast is
/// not a parameter. An unknown parameter is an error naming it.
pub fn bind(sql: &str, parameters: &Parameters) -> Result<(String, Vec<Option<String>>), String> {
    let mut text = String::with_capacity(sql.len());
    let mut names: Vec<&str> = Vec::new();
    let mut values = Vec::new();
    let bytes = sql.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let starts = bytes[i] == b':'
            && bytes
                .get(i + 1)
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
            && (i == 0 || bytes[i - 1] != b':');
        if !starts {
            let next = sql[i..].chars().next().map_or(1, char::len_utf8);
            text.push_str(&sql[i..i + next]);
            i += next;
            continue;
        }
        let end = sql[i + 1..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map_or(sql.len(), |n| i + 1 + n);
        let name = &sql[i + 1..end];
        let position = match names.iter().position(|n| *n == name) {
            Some(p) => p,
            None => {
                values.push(parameters.value(name)?);
                names.push(name);
                names.len() - 1
            }
        };
        text.push_str(&format!("${}", position + 1));
        i = end;
    }
    Ok((text, values))
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: Parameters = Parameters {
        dataset: "loans",
        cut: "2026-03-31",
        previous_cut: None,
    };

    #[test]
    fn named_parameters_are_bound_once_and_casts_stay() {
        let (text, values) = bind(
            "SELECT 'á' WHERE d = :dataset AND c = :cut::date AND x::text = :dataset",
            &AT,
        )
        .unwrap();
        assert_eq!(
            text,
            "SELECT 'á' WHERE d = $1 AND c = $2::date AND x::text = $1"
        );
        assert_eq!(
            values,
            vec![Some("loans".to_string()), Some("2026-03-31".to_string())]
        );
        assert_eq!(bind("SELECT :previous_cut", &AT).unwrap().1, vec![None]);
        assert_eq!(bind("SELECT :today", &AT).unwrap_err(), "today");
    }

    #[test]
    fn every_chart_binds_and_has_a_kind_the_web_draws() {
        for c in CATALOG {
            assert!(bind(c.sql, &AT).is_ok(), "{}", c.id);
            assert!(
                matches!(c.kind, "bar" | "line" | "scatter" | "table"),
                "{}",
                c.id
            );
            assert_eq!(chart(c.id), Some(c));
        }
        assert_eq!(chart("ghost"), None);
        let needing_nothing = CATALOG.iter().filter(|c| c.needs.is_empty()).count();
        assert_eq!(meeting(CATALOG, &[]).count(), needing_nothing);
        assert_eq!(
            meeting(
                CATALOG,
                &[
                    "entity",
                    "segment",
                    "updated",
                    "dimension",
                    "outlier_amount",
                    "drift",
                    "sfc"
                ]
            )
            .count(),
            CATALOG.len()
        );
    }

    #[test]
    fn a_chart_needing_roles_is_met_only_by_a_dataset_with_them_all() {
        static SOME: &[Chart] = &[
            Chart {
                id: "any",
                kind: "bar",
                needs: &[],
                sql: "",
            },
            Chart {
                id: "by_segment",
                kind: "bar",
                needs: &["entity", "segment"],
                sql: "",
            },
        ];
        let ids = |roles: &[&str]| meeting(SOME, roles).map(|c| c.id).collect::<Vec<_>>();
        assert_eq!(ids(&["segment", "entity", "drift"]), ["any", "by_segment"]);
        assert_eq!(ids(&["entity"]), ["any"]);
    }
}
