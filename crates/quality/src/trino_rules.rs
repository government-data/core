//! Trino's access-control rules (decision 0062, point 4), pure: the
//! datasets that read a source on Trino in, the rules Trino reads from the
//! API out (its file-based access control's JSON). The fixed part is by
//! pattern: people see `system` and queries, a read job's user
//! `src_<source>` reads the catalog of its name, the API's own client owns
//! everything, and anyone else gets nothing. Each dataset adds its
//! extraction's user, `_<dataset>`: its source's catalog read-only, and the
//! lake's schema `<dataset>_bronze`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::source::valid_name;
use crate::trino_catalog::catalog_name;
use crate::trino_extract::LAKE;

/// People: their e-mail (the Web UI's sign-in).
const PEOPLE: &str = ".*@.*";
/// A read job's user: its source's catalog.
const READ_JOBS: &str = "src_[a-z][a-z0-9_]*";
/// An extraction's user, whatever its dataset.
const EXTRACTIONS: &str = "_[a-z][a-z0-9_]*";

/// A dataset whose mapping reads `source`, a source on Trino.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Extracted {
    pub dataset: String,
    pub source: String,
}

/// The user Trino makes of an extraction's token: `_<dataset>`. It starts
/// with `_`, a read job's with `src_`, a person's has an `@`: none is
/// another's.
pub fn extraction_user(dataset: &str) -> String {
    format!("_{dataset}")
}

/// The lake's schema an extraction of `dataset` writes.
pub fn bronze_schema(dataset: &str) -> String {
    format!("{dataset}_bronze")
}

/// `text` as a regular expression that matches only itself.
fn exactly(text: &str) -> String {
    let mut pattern = String::with_capacity(text.len());
    for c in text.chars() {
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern
}

/// The rules: the fixed ones, the API's `client` as owner, and those of
/// each of `extracted`, in order of dataset. A dataset or a source whose
/// name is not `a-z`, `0-9`, `_` starting with a letter gets no rule: a
/// user is a regular expression, and such a name is no job's user.
pub fn rules(client: &str, extracted: &[Extracted]) -> Value {
    let client = exactly(client);
    let datasets: BTreeSet<&Extracted> = extracted
        .iter()
        .filter(|e| valid_name(&e.dataset) && valid_name(&e.source))
        .collect();
    let mut catalogs = vec![
        json!({ "user": PEOPLE, "catalog": "system", "allow": "read-only" }),
        json!({ "user": PEOPLE, "allow": "none" }),
        json!({ "user": READ_JOBS, "catalog": "{user}", "allow": "read-only" }),
    ];
    let mut tables = vec![
        json!({
            "user": client,
            "privileges": ["SELECT", "INSERT", "DELETE", "UPDATE", "OWNERSHIP", "GRANT_SELECT"]
        }),
        json!({ "user": PEOPLE, "catalog": "system", "privileges": ["SELECT"] }),
        json!({ "user": READ_JOBS, "catalog": "{user}", "privileges": ["SELECT"] }),
    ];
    for e in datasets {
        let user = extraction_user(&e.dataset);
        let source = catalog_name(&e.source);
        catalogs.push(json!({ "user": user, "catalog": source, "allow": "read-only" }));
        catalogs.push(json!({ "user": user, "catalog": LAKE, "allow": "all" }));
        tables.push(json!({ "user": user, "catalog": source, "privileges": ["SELECT"] }));
        // Creating a table needs OWNERSHIP (0062, Verification).
        tables.push(json!({
            "user": user,
            "catalog": LAKE,
            "schema": bronze_schema(&e.dataset),
            "privileges": ["SELECT", "INSERT", "DELETE", "OWNERSHIP"]
        }));
    }
    catalogs.push(json!({ "user": client, "allow": "owner" }));
    catalogs.push(json!({ "allow": "none" }));
    json!({
        "catalogs": catalogs,
        "schemas": [{ "user": client, "owner": true }],
        "tables": tables,
        "queries": [
            { "user": PEOPLE, "allow": ["view", "kill"] },
            { "user": client, "allow": ["execute", "view", "kill"] },
            { "user": format!("{READ_JOBS}|{EXTRACTIONS}"), "allow": ["execute"] }
        ],
        "system_information": [
            { "user": PEOPLE, "allow": ["read"] },
            { "user": client, "allow": ["read", "write"] }
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extracted(dataset: &str, source: &str) -> Extracted {
        Extracted {
            dataset: dataset.into(),
            source: source.into(),
        }
    }

    /// The rules of `user`, in `section`.
    fn of<'a>(rules: &'a Value, section: &str, user: &str) -> Vec<&'a Value> {
        rules[section]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["user"] == user)
            .collect()
    }

    #[test]
    fn the_verified_rules_are_the_ones_generated() {
        // What tools/verify/trino_rules_http.sh and trino_job_token.sh run
        // a Trino with.
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../tools/verify/fixtures/trino_rules.json"
        ))
        .unwrap();
        let generated = rules(
            "qd-api",
            &[extracted("orders", "alpha"), extracted("loans", "beta")],
        );
        assert_eq!(generated, fixture);
    }

    #[test]
    fn a_dataset_gets_its_source_and_its_bronze_schema_only() {
        let rules = rules("qd-api", &[extracted("orders", "alpha")]);
        assert_eq!(
            of(&rules, "catalogs", "_orders"),
            [
                &json!({ "user": "_orders", "catalog": "src_alpha", "allow": "read-only" }),
                &json!({ "user": "_orders", "catalog": "lake", "allow": "all" }),
            ]
        );
        let tables = of(&rules, "tables", "_orders");
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0]["privileges"], json!(["SELECT"]));
        assert_eq!(tables[1]["schema"], "orders_bronze");
        // Nothing of the lake but that schema, and nothing by pattern.
        let text = rules.to_string();
        assert_eq!(text.matches("_bronze").count(), 1);
        assert!(!text.contains("src{user}"));
    }

    #[test]
    fn without_datasets_only_the_fixed_rules_and_the_last_refuses_everyone() {
        let rules = rules("qd-api", &[]);
        let catalogs = rules["catalogs"].as_array().unwrap();
        assert_eq!(catalogs.len(), 5);
        assert_eq!(catalogs.last().unwrap(), &json!({ "allow": "none" }));
        assert!(rules["tables"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["catalog"] != "lake"));
    }

    #[test]
    fn datasets_come_in_order_once_and_a_bad_name_gets_no_rule() {
        let rules = rules(
            "qd-api",
            &[
                extracted("orders", "alpha"),
                extracted("loans", "beta"),
                extracted("orders", "alpha"),
                extracted("x|.*", "alpha"),
                extracted("Orders", "alpha"),
                extracted("clients", "al.*"),
                extracted("", "alpha"),
            ],
        );
        let users: Vec<&str> = rules["catalogs"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["user"].as_str())
            .filter(|u| u.starts_with('_'))
            .collect();
        assert_eq!(users, ["_loans", "_loans", "_orders", "_orders"]);
    }

    #[test]
    fn no_datasets_user_is_a_read_jobs_a_persons_or_the_apis() {
        for dataset in ["src_core", "src", "qd", "a"] {
            let user = extraction_user(dataset);
            assert!(user.starts_with('_') && !user.starts_with("src_") && !user.contains('@'));
        }
        // A dataset named as a source's catalog is still its own user.
        let rules = rules("qd-api", &[extracted("src_core", "core")]);
        assert_eq!(of(&rules, "catalogs", "_src_core").len(), 2);
        assert!(of(&rules, "catalogs", "src_core").is_empty());
    }

    #[test]
    fn the_apis_client_is_matched_exactly() {
        let rules = rules("qd.api+x", &[]);
        assert_eq!(rules["schemas"][0]["user"], r"qd\.api\+x");
        assert_eq!(of(&rules, "catalogs", r"qd\.api\+x").len(), 1);
    }
}
