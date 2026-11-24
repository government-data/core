//! The modules' actions and managed policies in IAM (quality-sdk decision
//! 0009): as the update agent tells them with the installed modules, shown
//! and assigned as the core's own. A module's managed policy allows its
//! actions on `*`; it cannot be changed or removed here. A removed module's
//! actions, policies and assignments stay, shown as a removed module's; its
//! policies grant nothing while it is removed. Only a purge deletes them.

use iam::{Policy, Statement};
use serde::Serialize;

use super::{PlatformError, PlatformService, R};
use crate::DeclaredModule;

/// The module an action or a managed policy belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OfModule {
    pub name: String,
    /// Whether it is installed now; a removed module's stay.
    pub installed: bool,
}

/// A module's action, for the catalog: its description in the request's
/// language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModuleActionView {
    pub name: String,
    pub description: String,
    pub module: OfModule,
}

impl PlatformService {
    /// The modules' permissions the core keeps; none without the updates.
    pub(super) async fn declared_modules(&self) -> R<Vec<DeclaredModule>> {
        match &self.updates {
            Some(updates) => updates
                .declared_modules()
                .await
                .map_err(PlatformError::Internal),
            None => Ok(Vec::new()),
        }
    }

    /// Every module's actions, a removed module's included, in the
    /// request's language.
    pub async fn module_actions(&self) -> R<Vec<ModuleActionView>> {
        let language = texts::current().code();
        Ok(self
            .declared_modules()
            .await?
            .iter()
            .flat_map(|m| {
                m.permissions.actions.iter().map(|a| ModuleActionView {
                    name: a.name.clone(),
                    description: a.description.get(language).to_string(),
                    module: of(m),
                })
            })
            .collect())
    }
}

/// The module `m` as an owner.
pub(super) fn of(m: &DeclaredModule) -> OfModule {
    OfModule {
        name: m.name.clone(),
        installed: m.installed,
    }
}

/// The managed policies of `modules`, with their module, in `language`.
pub(super) fn policies(modules: &[DeclaredModule], language: &str) -> Vec<(Policy, OfModule)> {
    modules
        .iter()
        .flat_map(|m| {
            m.permissions.policies.iter().map(move |p| {
                let actions: Vec<&str> = p.actions.iter().map(String::as_str).collect();
                let policy = Policy {
                    name: p.name.clone(),
                    description: p.description.get(language).to_string(),
                    statements: vec![Statement::allow(&actions, &["*"])],
                    managed: true,
                };
                (policy, of(m))
            })
        })
        .collect()
}

/// The managed policy named `name` of an installed module: a removed
/// module's grants nothing.
pub(super) fn granting(modules: &[DeclaredModule], name: &str) -> Option<Policy> {
    policies(modules, "en")
        .into_iter()
        .find(|(p, m)| m.installed && p.name == name)
        .map(|(p, _)| p)
}

/// Whether `name` is a module's managed policy, installed or removed.
pub(super) fn is_module_policy(modules: &[DeclaredModule], name: &str) -> bool {
    modules
        .iter()
        .any(|m| m.permissions.policies.iter().any(|p| p.name == name))
}

/// Every module's action names, for checking a custom policy.
pub(super) fn action_names(modules: &[DeclaredModule]) -> Vec<&str> {
    modules
        .iter()
        .flat_map(|m| m.permissions.actions.iter().map(|a| a.name.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InLanguages, ModuleAction, ModulePermissions, ModulePolicy};

    fn bi(installed: bool) -> DeclaredModule {
        let text = |t: &str| InLanguages {
            en: t.into(),
            es: format!("{t} (es)"),
        };
        DeclaredModule {
            name: "bi-superset".into(),
            installed,
            permissions: ModulePermissions {
                actions: vec![ModuleAction {
                    name: "bi-superset:Author".into(),
                    description: text("Create charts"),
                }],
                policies: vec![ModulePolicy {
                    name: "bi-superset:Authors".into(),
                    description: text("BI authors"),
                    actions: vec!["bi-superset:Author".into()],
                }],
            },
        }
    }

    #[test]
    fn an_installed_modules_policy_allows_its_actions_on_everything() {
        let modules = [bi(true)];
        let policy = granting(&modules, "bi-superset:Authors").unwrap();
        assert!(policy.managed);
        let p = iam::Permissions::from_policies(&[policy]);
        assert!(p.can("bi-superset:Author", "*"));
        assert!(!p.can("quality:ViewRules", "*"));
        assert_eq!(policies(&modules, "es")[0].0.description, "BI authors (es)");
        assert_eq!(action_names(&modules), ["bi-superset:Author"]);
    }

    #[test]
    fn a_removed_modules_policy_stays_and_grants_nothing() {
        let modules = [bi(false)];
        assert!(granting(&modules, "bi-superset:Authors").is_none());
        assert!(is_module_policy(&modules, "bi-superset:Authors"));
        assert!(!policies(&modules, "en")[0].1.installed);
        assert!(!is_module_policy(&modules, "quality-admin"));
    }
}
