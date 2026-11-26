//! The people's neutral grants (decision 0089, step 5; ruling 23): what
//! each person may read of gold, in the shape of the job users' grants
//! ([`crate::job_grants::Grant`]), for `engine-trino`'s rules merge to
//! write as Trino rules with a row filter. A person may read gold's rows of
//! the datasets they may `quality:ViewResults` on (`pipeline/<dataset>`),
//! as the platform's IAM answers through [`contracts::Grantees`]. Names
//! and account ids only.

use contracts::Grantees;

use crate::job_grants::{is_name, Grant, Privilege, Rows, GOLD};

/// The action that gives a dataset's results.
pub const VIEW: &str = "quality:ViewResults";

/// What people read in gold (decision 0090, ruling 73): its results and
/// their current-run views, and the entities only through the aggregate
/// views; never `entity_icd` (one row per entity), its view or the jobs'
/// counts.
pub const PEOPLE_GOLD: &[&str] = &[
    "run_summary",
    "rule_results",
    "field_quality",
    "group_metrics",
    "distribution_checks",
    "cells",
    "queue_findings",
    "findings",
    "finding_groups",
    "remediation_slas",
    "current_run",
    "v_run_summary",
    "v_rule_results",
    "v_field_quality",
    "v_group_metrics",
    "v_distribution_checks",
    "v_cells",
    "v_queue_findings",
    "v_findings",
    "v_finding_groups",
    "v_icd_distribution",
    "v_band_transitions",
    "v_bands_by_segment",
];

/// The resource of a dataset.
fn resource(dataset: &str) -> String {
    format!("pipeline/{dataset}")
}

/// One grant per person who may view the results of at least one of
/// `datasets`: `{subject: person-<account>, namespace: quality_gold,
/// tables: [PEOPLE_GOLD], privileges: [SELECT], rows: {column: dataset,
/// in: [...]}}`. A dataset whose name the merge would refuse gets no grant.
pub async fn people_grants(
    grantees: &dyn Grantees,
    datasets: &[String],
) -> Result<Vec<Grant>, String> {
    let named: Vec<&String> = datasets.iter().filter(|d| is_name(d)).collect();
    let resources: Vec<String> = named.iter().map(|d| resource(d)).collect();
    let allowed = grantees.allowed(VIEW, &resources).await?;
    Ok(allowed
        .into_iter()
        .filter_map(|g| {
            let values: Vec<String> = named
                .iter()
                .filter(|d| g.resources.contains(&resource(d)))
                .map(|d| (*d).clone())
                .collect();
            (!values.is_empty()).then(|| Grant {
                subject: format!("person-{}", g.account),
                namespace: GOLD.into(),
                tables: PEOPLE_GOLD.iter().map(|t| (*t).to_string()).collect(),
                privileges: vec![Privilege::Select],
                rows: Some(Rows {
                    column: "dataset".into(),
                    values,
                }),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use contracts::Grantee;

    use super::*;

    /// Ana may view `ventas`, Bea every dataset.
    struct Two;

    #[async_trait]
    impl Grantees for Two {
        async fn allowed(
            &self,
            action: &str,
            resources: &[String],
        ) -> Result<Vec<Grantee>, String> {
            assert_eq!(action, VIEW);
            let ana = resources.iter().filter(|r| *r == "pipeline/ventas");
            Ok(vec![
                Grantee {
                    account: "a1".into(),
                    resources: ana.cloned().collect(),
                },
                Grantee {
                    account: "b2".into(),
                    resources: resources.to_vec(),
                },
            ])
        }
    }

    #[tokio::test]
    async fn each_person_reads_gold_s_rows_of_the_datasets_they_may_view() {
        let datasets = ["ventas", "compras", "Not-A-Name"].map(String::from);
        let grants = people_grants(&Two, &datasets).await.unwrap();
        let shown: Vec<(String, Vec<String>)> = grants
            .iter()
            .map(|g| (g.subject.clone(), g.rows.clone().unwrap().values))
            .collect();
        assert_eq!(
            shown,
            [
                ("person-a1".to_string(), vec!["ventas".to_string()]),
                (
                    "person-b2".to_string(),
                    vec!["ventas".to_string(), "compras".to_string()]
                ),
            ]
        );
        let tables = &grants[0].tables;
        assert!(!tables
            .iter()
            .any(|t| t.contains("entity") || t == "job_counts" || t == "*"));
        let json = serde_json::to_value(&grants[0]).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "subject": "person-a1", "namespace": "quality_gold",
                "tables": PEOPLE_GOLD, "privileges": ["SELECT"],
                "rows": { "column": "dataset", "in": ["ventas"] } })
        );
    }
}
