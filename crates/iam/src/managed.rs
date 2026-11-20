//! The policies the application ships: an administrator attaches them to
//! groups without writing statements. They cannot be changed or removed, and
//! their names are reserved. RustFS has policies of the same names for the
//! storage side (ADR 0016).

use crate::catalog::{ACTIONS, VIEW_AUDIT, VIEW_PERSONAL_DATA};
use crate::policy::{Policy, Statement};

/// The names, in display order.
pub const NAMES: [&str; 5] = [
    "quality-admin",
    "quality-steward",
    "quality-analyst",
    "quality-viewer",
    "auditor",
];

fn quality(keep: impl Fn(&str) -> bool) -> Vec<&'static str> {
    ACTIONS
        .iter()
        .map(|a| a.name)
        .filter(|n| n.starts_with("quality:") && keep(n))
        .collect()
}

fn is_view(name: &str) -> bool {
    name.starts_with("quality:View")
}

/// The managed policy named `name`.
pub fn find(name: &str) -> Option<Policy> {
    let (description, actions): (&str, Vec<&str>) = match name {
        "quality-admin" => ("Everything, on every resource.", vec!["*"]),
        "quality-steward" => (
            "Whoever owns the rules and the pipelines: every quality action except seeing personal data.",
            quality(|n| n != VIEW_PERSONAL_DATA),
        ),
        "quality-analyst" => (
            "Whoever investigates findings: sees everything, personal data included, runs pipelines and exports results.",
            quality(|n| is_view(n) || n == "quality:RunPipeline" || n == "quality:ExportResults"),
        ),
        "quality-viewer" => (
            "Managers and dashboards: pipelines, results and rules, without personal data.",
            vec!["quality:ViewPipelines", "quality:ViewResults", "quality:ViewRules"],
        ),
        "auditor" => (
            "Internal control: sees everything and the audit log, without personal data.",
            {
                let mut a = quality(|n| is_view(n) && n != VIEW_PERSONAL_DATA);
                a.push(VIEW_AUDIT);
                a
            },
        ),
        _ => return None,
    };
    Some(Policy {
        name: name.to_string(),
        description: description.to_string(),
        statements: vec![Statement::allow(&actions, &["*"])],
        managed: true,
    })
}

/// Every managed policy, in display order.
pub fn all() -> Vec<Policy> {
    NAMES.iter().filter_map(|n| find(n)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Permissions;

    fn grants(policy: &str, action: &str) -> bool {
        Permissions::from_policies(&[find(policy).unwrap()]).can(action, "pipeline/p")
    }

    #[test]
    fn what_each_one_grants() {
        assert_eq!(all().len(), NAMES.len());
        assert!(grants("quality-admin", "admin:ManagePolicies"));
        assert!(grants("quality-admin", crate::catalog::MANAGE_MACHINES));
        assert!(!grants("auditor", crate::catalog::MANAGE_MACHINES));
        assert!(grants("quality-steward", "quality:EditRules"));
        assert!(!grants("quality-steward", VIEW_PERSONAL_DATA));
        assert!(grants("quality-analyst", VIEW_PERSONAL_DATA));
        assert!(grants("quality-analyst", "quality:RunPipeline"));
        assert!(!grants("quality-analyst", "quality:EditPipelines"));
        assert!(grants("quality-viewer", "quality:ViewResults"));
        assert!(!grants("quality-viewer", VIEW_PERSONAL_DATA));
        assert!(!grants("quality-viewer", "quality:RunPipeline"));
        assert!(grants("auditor", "quality:ViewSources"));
        assert!(!grants("auditor", VIEW_PERSONAL_DATA));
        assert!(Permissions::from_policies(&[find("auditor").unwrap()]).can(VIEW_AUDIT, "*"));
    }

    #[test]
    fn a_steward_with_analyst_sees_personal_data() {
        // No denies in the managed ones: attaching both adds up.
        let p = Permissions::from_policies(&[
            find("quality-steward").unwrap(),
            find("quality-analyst").unwrap(),
        ]);
        assert!(p.can(VIEW_PERSONAL_DATA, "pipeline/p"));
    }
}
