//! The actions the application knows, and the kind of resource each one acts
//! on.

/// The kinds of resource, as the first part of a resource name
/// (`secret/BD_INFORMES`).
pub const RESOURCE_KINDS: [&str; 6] = ["source", "etl", "dataset", "catalog", "pipeline", "secret"];

/// An action of the catalog: `service:Verb`, and the kind of resource it acts
/// on (`None`: it acts on the installation as a whole, whose resource is `*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Action {
    pub name: &'static str,
    pub resource: Option<&'static str>,
}

const fn on(name: &'static str, kind: &'static str) -> Action {
    Action {
        name,
        resource: Some(kind),
    }
}

const fn global(name: &'static str) -> Action {
    Action {
        name,
        resource: None,
    }
}

/// Every action, in display order.
pub const ACTIONS: [Action; 26] = [
    on("quality:ViewSources", "source"),
    on("quality:ManageSources", "source"),
    on("quality:ViewEtl", "etl"),
    on("quality:EditEtl", "etl"),
    on("quality:ViewDatasets", "dataset"),
    on("quality:ManageDatasets", "dataset"),
    on("quality:ViewRules", "catalog"),
    on("quality:EditRules", "catalog"),
    // A draft of the catalog merged into `main` (decision 0021).
    on("quality:ApproveRules", "catalog"),
    on("quality:ViewPipelines", "pipeline"),
    on("quality:EditPipelines", "pipeline"),
    on("quality:RunPipeline", "pipeline"),
    on("quality:ManageSchedules", "pipeline"),
    on("quality:ViewResults", "pipeline"),
    on("quality:ExportResults", "pipeline"),
    on("quality:ViewPersonalData", "pipeline"),
    global("quality:QueryData"),
    on("quality:ManageSubscriptions", "pipeline"),
    global("admin:Configure"),
    global("admin:ManageUsers"),
    global("admin:ManagePolicies"),
    on("admin:ManageSecrets", "secret"),
    global("admin:ViewAudit"),
    global("admin:ManageLicense"),
    global("admin:ManageUpdates"),
    // The installation's machines: join tokens, approving, draining and
    // retiring them (decisions 0029 and 0066).
    global("admin:ManageMachines"),
];

/// The action named `name`, if the catalog has it.
pub fn action(name: &str) -> Option<Action> {
    ACTIONS.iter().copied().find(|a| a.name == name)
}

// The names the code checks, so a typo is a compile error.
pub const CONFIGURE: &str = "admin:Configure";
pub const MANAGE_USERS: &str = "admin:ManageUsers";
pub const MANAGE_POLICIES: &str = "admin:ManagePolicies";
pub const MANAGE_SECRETS: &str = "admin:ManageSecrets";
pub const VIEW_AUDIT: &str = "admin:ViewAudit";
pub const MANAGE_LICENSE: &str = "admin:ManageLicense";
pub const MANAGE_UPDATES: &str = "admin:ManageUpdates";
pub const MANAGE_MACHINES: &str = "admin:ManageMachines";
pub const VIEW_PERSONAL_DATA: &str = "quality:ViewPersonalData";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_kinds_known() {
        for (i, a) in ACTIONS.iter().enumerate() {
            assert!(ACTIONS[..i].iter().all(|b| b.name != a.name), "{}", a.name);
            assert!(a.resource.is_none_or(|k| RESOURCE_KINDS.contains(&k)));
        }
        for name in [
            CONFIGURE,
            MANAGE_USERS,
            MANAGE_POLICIES,
            MANAGE_SECRETS,
            VIEW_AUDIT,
            MANAGE_LICENSE,
            MANAGE_UPDATES,
            MANAGE_MACHINES,
            VIEW_PERSONAL_DATA,
        ] {
            assert!(action(name).is_some(), "{name}");
        }
    }
}
