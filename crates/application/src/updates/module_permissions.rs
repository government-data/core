//! A module's actions and managed policies, and the action its menu entry
//! requires, as the update agent tells them with the installed modules
//! (quality-sdk decision 0009). Module actions act on the installation as a
//! whole (resource `*`); each action and policy has the module's name as
//! its prefix (`bi-superset:Author`, `bi-superset:Authors`).

use serde::{Deserialize, Serialize};

use super::module_menu::InLanguages;

/// The longest description taken.
const MAX_DESCRIPTION: usize = 200;
/// The longest name after the prefix (`Author`).
const MAX_VERB: usize = 64;
/// The most actions, and the most policies, a module may declare.
const MAX_ENTRIES: usize = 64;

/// What a module declares in its manifest's `permissions`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModulePermissions {
    #[serde(default)]
    pub actions: Vec<ModuleAction>,
    #[serde(default)]
    pub policies: Vec<ModulePolicy>,
}

/// One of a module's actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleAction {
    pub name: String,
    pub description: InLanguages,
}

/// One of a module's managed policies: it allows its actions on `*`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModulePolicy {
    pub name: String,
    pub description: InLanguages,
    pub actions: Vec<String>,
}

/// Why a module's permissions or menu condition are refused (the
/// message's key and the name at fault).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    /// An action or policy whose prefix is not the module's name, or a
    /// name not of the form `<prefix>:<Verb>`.
    ForeignName(String),
    /// An action of the core's catalog declared as the module's own.
    CoreAction(String),
    /// A name declared twice, or too many of them.
    Repeated(String),
    /// A description empty, too long or with control characters.
    BadDescription(String),
    /// A policy or menu condition naming an action that is neither the
    /// core's nor the module's own.
    UnknownAction(String),
}

/// `<prefix>:<Verb>`: the parts, when `name` has that form.
pub(super) fn split(name: &str) -> Option<(&str, &str)> {
    let (prefix, verb) = name.split_once(':')?;
    let mut chars = verb.chars();
    let verb_ok = verb.len() <= MAX_VERB
        && chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_alphanumeric());
    let prefix_ok = !prefix.is_empty()
        && prefix
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    (verb_ok && prefix_ok).then_some((prefix, verb))
}

/// Whether `action` is an action of the core's catalog or of `own`.
pub(super) fn is_known(action: &str, own: &ModulePermissions) -> bool {
    iam::catalog::action(action).is_some() || own.actions.iter().any(|a| a.name == action)
}

/// Check the permissions of the module `module` and the action its menu
/// entry `requires`.
pub(super) fn check(
    module: &str,
    permissions: Option<&ModulePermissions>,
    requires: Option<&str>,
) -> Result<(), Refusal> {
    let empty = ModulePermissions::default();
    let own = permissions.unwrap_or(&empty);
    check_actions(module, own)?;
    check_policies(module, own)?;
    match requires {
        Some(action) if split(action).is_none() || !is_known(action, own) => {
            Err(Refusal::UnknownAction(action.to_string()))
        }
        _ => Ok(()),
    }
}

fn check_actions(module: &str, own: &ModulePermissions) -> Result<(), Refusal> {
    let mut seen = std::collections::BTreeSet::new();
    for a in &own.actions {
        own_name(module, &a.name)?;
        if iam::catalog::action(&a.name).is_some() {
            return Err(Refusal::CoreAction(a.name.clone()));
        }
        if !seen.insert(a.name.as_str()) || seen.len() > MAX_ENTRIES {
            return Err(Refusal::Repeated(a.name.clone()));
        }
        description(&a.name, &a.description)?;
    }
    Ok(())
}

fn check_policies(module: &str, own: &ModulePermissions) -> Result<(), Refusal> {
    let mut seen = std::collections::BTreeSet::new();
    for p in &own.policies {
        own_name(module, &p.name)?;
        if !seen.insert(p.name.as_str()) || seen.len() > MAX_ENTRIES {
            return Err(Refusal::Repeated(p.name.clone()));
        }
        description(&p.name, &p.description)?;
        if p.actions.is_empty() {
            return Err(Refusal::UnknownAction(p.name.clone()));
        }
        if let Some(a) = p.actions.iter().find(|a| !is_known(a, own)) {
            return Err(Refusal::UnknownAction(a.clone()));
        }
    }
    Ok(())
}

fn own_name(module: &str, name: &str) -> Result<(), Refusal> {
    match split(name) {
        Some((prefix, _)) if prefix == module => Ok(()),
        _ => Err(Refusal::ForeignName(name.to_string())),
    }
}

fn description(name: &str, d: &InLanguages) -> Result<(), Refusal> {
    let ok = [&d.en, &d.es].iter().all(|t| {
        let t = t.trim();
        !t.is_empty() && t.chars().count() <= MAX_DESCRIPTION && !t.chars().any(char::is_control)
    });
    if ok {
        Ok(())
    } else {
        Err(Refusal::BadDescription(name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> InLanguages {
        InLanguages {
            en: t.into(),
            es: t.into(),
        }
    }

    fn action(name: &str) -> ModuleAction {
        ModuleAction {
            name: name.into(),
            description: text("Create charts"),
        }
    }

    fn policy(name: &str, actions: &[&str]) -> ModulePolicy {
        ModulePolicy {
            name: name.into(),
            description: text("BI authors"),
            actions: actions.iter().map(|a| a.to_string()).collect(),
        }
    }

    fn superset() -> ModulePermissions {
        ModulePermissions {
            actions: vec![
                action("bi-superset:Author"),
                action("bi-superset:Administer"),
            ],
            policies: vec![
                policy("bi-superset:Authors", &["bi-superset:Author"]),
                policy(
                    "bi-superset:Administrators",
                    &["bi-superset:Author", "bi-superset:Administer"],
                ),
            ],
        }
    }

    #[test]
    fn a_modules_own_actions_and_policies_are_taken() {
        let p = superset();
        assert_eq!(check("bi-superset", Some(&p), None), Ok(()));
        assert_eq!(
            check("bi-superset", Some(&p), Some("bi-superset:Author")),
            Ok(())
        );
        assert_eq!(
            check("bi-superset", Some(&p), Some("quality:ViewResults")),
            Ok(())
        );
        assert_eq!(check("docs", None, None), Ok(()));
        let mut with_core = superset();
        with_core.policies[0]
            .actions
            .push("quality:ViewResults".into());
        assert_eq!(check("bi-superset", Some(&with_core), None), Ok(()));
    }

    #[test]
    fn a_foreign_prefix_or_a_core_action_is_refused() {
        let mut foreign = superset();
        foreign.actions.push(action("lake-rustfs:Author"));
        assert_eq!(
            check("bi-superset", Some(&foreign), None),
            Err(Refusal::ForeignName("lake-rustfs:Author".into()))
        );
        let mut foreign_policy = superset();
        foreign_policy.policies.push(policy("other:Authors", &[]));
        assert!(matches!(
            check("bi-superset", Some(&foreign_policy), None),
            Err(Refusal::ForeignName(_))
        ));
        let core = ModulePermissions {
            actions: vec![action("quality:ViewRules")],
            policies: vec![],
        };
        assert_eq!(
            check("quality", Some(&core), None),
            Err(Refusal::CoreAction("quality:ViewRules".into()))
        );
        for bad in [
            "bi-superset:author",
            "bi-superset:",
            "bi-superset",
            ":Author",
        ] {
            let p = ModulePermissions {
                actions: vec![action(bad)],
                policies: vec![],
            };
            assert!(
                matches!(
                    check("bi-superset", Some(&p), None),
                    Err(Refusal::ForeignName(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn unknown_actions_repeats_and_bad_descriptions_are_refused() {
        let mut unknown = superset();
        unknown.policies[0].actions.push("other:Author".into());
        assert_eq!(
            check("bi-superset", Some(&unknown), None),
            Err(Refusal::UnknownAction("other:Author".into()))
        );
        assert_eq!(
            check("bi-superset", Some(&superset()), Some("bi-superset:Fly")),
            Err(Refusal::UnknownAction("bi-superset:Fly".into()))
        );
        assert!(check("docs", None, Some("not an action")).is_err());
        let mut twice = superset();
        twice.actions.push(action("bi-superset:Author"));
        assert!(matches!(
            check("bi-superset", Some(&twice), None),
            Err(Refusal::Repeated(_))
        ));
        let mut blank = superset();
        blank.actions[0].description.es = " ".into();
        assert!(matches!(
            check("bi-superset", Some(&blank), None),
            Err(Refusal::BadDescription(_))
        ));
        let mut empty = superset();
        empty.policies[0].actions.clear();
        assert!(check("bi-superset", Some(&empty), None).is_err());
    }
}
