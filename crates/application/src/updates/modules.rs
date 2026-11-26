//! The installed modules, as the update agent tells them (decision 0003):
//! the whole list each time (`PUT /updater/v1/modules`), each with its
//! version and, when its manifest declares one, its drain path. The drain
//! asks every module that has a drain path ([`UpdatesService::installed_modules`]).
//! A module with a route also tells its path and, with a menu entry, the
//! entry the apps menu shows ([`super::module_menu`]).

use serde::{Deserialize, Serialize};

use super::module_menu::{self, ModuleMenu, Refusal};
use super::module_permissions::{self, ModulePermissions};
use super::module_roles::{self, ModuleRole};
use super::{UpdatesError, UpdatesService};

/// The longest name, version or drain path taken.
const MAX_LEN: usize = 200;

/// The most capabilities a module may say it provides.
const MAX_PROVIDES: usize = 16;

/// An installed module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledModule {
    /// Its folder's name: lower-case letters, digits and `-`.
    pub name: String,
    pub version: String,
    /// Where it answers its drain (`/…`), `None` when it declares none.
    #[serde(default)]
    pub drain_path: Option<String>,
    /// The path its route serves on the web entry point (`/docs`), `None`
    /// without a route.
    #[serde(default)]
    pub route_path: Option<String>,
    /// Its entry in the apps menu, `None` without one.
    #[serde(default)]
    pub menu: Option<ModuleMenu>,
    /// Its actions and managed policies, `None` when it declares none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<ModulePermissions>,
    /// The capabilities it provides (`lake`), as its manifest names them:
    /// how the core knows a module's data is the one it replicates
    /// without naming the module (decision 0078).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provides: Vec<String>,
    /// Its machine roles, with their images and files (quality-sdk
    /// decision 0013), none when it declares none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<ModuleRole>,
    /// Where it answers whether a module may be removed (quality-sdk
    /// decision 0015), `None` when it declares none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removal_path: Option<String>,
}

impl UpdatesService {
    /// Replace the installed modules with `modules` (the agent sends the
    /// whole list; sending it again changes nothing).
    pub async fn set_modules(&self, modules: Vec<InstalledModule>) -> Result<(), UpdatesError> {
        check(&modules).map_err(|e| UpdatesError::Invalid("modules", e))?;
        self.store
            .save_modules(&modules)
            .await
            .map_err(UpdatesError::Unavailable)
    }

    /// The installed modules as last told; none before the agent tells.
    pub async fn installed_modules(&self) -> Result<Vec<InstalledModule>, String> {
        self.store.modules().await
    }

    /// Every module's permissions the core keeps, a removed module's
    /// included until it is purged (quality-sdk decision 0009).
    pub async fn declared_modules(&self) -> Result<Vec<DeclaredModule>, String> {
        let installed: std::collections::BTreeSet<String> = self
            .store
            .modules()
            .await?
            .into_iter()
            .map(|m| m.name)
            .collect();
        let kept = self.store.module_permissions().await?;
        Ok(kept
            .into_iter()
            .map(|(name, permissions)| DeclaredModule {
                installed: installed.contains(&name),
                name,
                permissions,
            })
            .collect())
    }
}

impl UpdatesService {
    /// Forget a module's permissions (its purge): its actions leave the
    /// catalog. Its policies' assignments are the platform's to delete.
    pub async fn forget_module_permissions(&self, module: &str) -> Result<(), String> {
        self.store.forget_module_permissions(module).await
    }
}

/// A module's actions and managed policies as the core keeps them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredModule {
    pub name: String,
    /// Whether the installed list names it now.
    pub installed: bool,
    pub permissions: ModulePermissions,
}

fn check(modules: &[InstalledModule]) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for m in modules {
        if !is_name(&m.name) {
            return Err(texts::tr!("app.updates.modules.badName"));
        }
        if !seen.insert(m.name.as_str()) {
            return Err(texts::tr!("app.updates.modules.repeated", name = m.name));
        }
        let version = m.version.trim();
        if version.is_empty() || version.len() > MAX_LEN || version.chars().any(char::is_control) {
            return Err(texts::tr!("app.updates.modules.badVersion", name = m.name));
        }
        if let Some(path) = &m.drain_path {
            if !is_drain_path(path) {
                return Err(texts::tr!(
                    "app.updates.modules.badDrainPath",
                    name = m.name
                ));
            }
        }
        if m.removal_path.as_deref().is_some_and(|p| !is_drain_path(p)) {
            return Err(texts::tr!(
                "app.updates.modules.badRemovalPath",
                name = m.name
            ));
        }
        if m.provides.len() > MAX_PROVIDES || !m.provides.iter().all(|c| is_name(c)) {
            return Err(texts::tr!("app.updates.modules.badProvides", name = m.name));
        }
        match module_menu::check(m.route_path.as_deref(), m.menu.as_ref()) {
            Ok(()) => {}
            Err(Refusal::BadRoutePath) => {
                return Err(texts::tr!(
                    "app.updates.modules.badRoutePath",
                    name = m.name
                ))
            }
            Err(Refusal::MenuWithoutRoute) => {
                return Err(texts::tr!(
                    "app.updates.modules.menuWithoutRoute",
                    name = m.name
                ))
            }
            Err(Refusal::BadMenu) => {
                return Err(texts::tr!("app.updates.modules.badMenu", name = m.name))
            }
        }
        let requires = m.menu.as_ref().and_then(|menu| menu.requires.as_deref());
        module_permissions::check(&m.name, m.permissions.as_ref(), requires)
            .map_err(|refusal| permissions_refusal(&m.name, refusal))?;
        module_roles::check(&m.roles).map_err(|refusal| roles_refusal(&m.name, refusal))?;
    }
    Ok(())
}

fn roles_refusal(module: &str, refusal: module_roles::Refusal) -> String {
    use module_roles::Refusal as R;
    match refusal {
        R::TooMany => texts::tr!("app.updates.modules.tooManyRoles", name = module),
        R::BadName(role) => texts::tr!("app.updates.modules.badRole", name = module, role = role),
        R::Reserved(role) => {
            texts::tr!(
                "app.updates.modules.reservedRole",
                name = module,
                role = role
            )
        }
        R::Repeated(role) => {
            texts::tr!(
                "app.updates.modules.repeatedRole",
                name = module,
                role = role
            )
        }
        R::BadServices(role) => {
            texts::tr!(
                "app.updates.modules.badRoleServices",
                name = module,
                role = role
            )
        }
        R::BadImages(role) => {
            texts::tr!(
                "app.updates.modules.badRoleImages",
                name = module,
                role = role
            )
        }
        R::BadFile(role) => {
            texts::tr!(
                "app.updates.modules.badRoleFile",
                name = module,
                role = role
            )
        }
        R::FilesTooLarge => texts::tr!("app.updates.modules.roleFilesTooLarge", name = module),
    }
}

fn permissions_refusal(module: &str, refusal: module_permissions::Refusal) -> String {
    use module_permissions::Refusal as R;
    match refusal {
        R::ForeignName(n) => {
            texts::tr!("app.updates.modules.foreignName", name = module, action = n)
        }
        R::CoreAction(n) => texts::tr!("app.updates.modules.coreAction", name = module, action = n),
        R::Repeated(n) => texts::tr!(
            "app.updates.modules.repeatedAction",
            name = module,
            action = n
        ),
        R::BadDescription(n) => {
            texts::tr!(
                "app.updates.modules.badDescription",
                name = module,
                action = n
            )
        }
        R::UnknownAction(n) => {
            texts::tr!(
                "app.updates.modules.unknownAction",
                name = module,
                action = n
            )
        }
    }
}

pub(super) fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= MAX_LEN
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// An absolute path with no query, no fragment and no `..` or empty
/// segment, so that it stays under the module's own prefix.
fn is_drain_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix('/') else {
        return false;
    };
    path.len() <= MAX_LEN
        && !rest.is_empty()
        && rest
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != "..")
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::updates::tests::MemoryMaintenance;

    fn module(name: &str, drain_path: Option<&str>) -> InstalledModule {
        InstalledModule {
            name: name.into(),
            version: "1.0.0".into(),
            drain_path: drain_path.map(Into::into),
            route_path: None,
            menu: None,
            permissions: None,
            provides: Vec::new(),
            roles: Vec::new(),
            removal_path: None,
        }
    }

    #[tokio::test]
    async fn a_modules_roles_are_kept_and_a_bad_list_keeps_the_one_before() {
        use crate::updates::module_roles::tests::{file, role};
        let service = UpdatesService::new(Arc::new(MemoryMaintenance::default()));
        let mut lake = role("lake");
        lake.files = vec![file("compose.yml", b"services: {}\n")];
        let kept = vec![InstalledModule {
            roles: vec![lake],
            ..module("lake-rustfs", None)
        }];
        service.set_modules(kept.clone()).await.unwrap();
        assert_eq!(service.installed_modules().await.unwrap(), kept);
        let refused = service
            .set_modules(vec![InstalledModule {
                roles: vec![role("app")],
                ..module("lake-rustfs", None)
            }])
            .await;
        assert!(matches!(refused, Err(UpdatesError::Invalid("modules", _))));
        assert_eq!(service.installed_modules().await.unwrap(), kept, "kept");
        let older = serde_json::json!({"name": "docs", "version": "1.0.0"});
        let older: InstalledModule = serde_json::from_value(older).unwrap();
        assert!(older.roles.is_empty(), "an agent that does not send them");
    }

    #[tokio::test]
    async fn a_module_says_the_capabilities_it_provides() {
        let service = UpdatesService::new(Arc::new(MemoryMaintenance::default()));
        let lake = InstalledModule {
            provides: vec!["lake".into()],
            ..module("lake-rustfs", None)
        };
        service.set_modules(vec![lake.clone()]).await.unwrap();
        assert_eq!(service.installed_modules().await.unwrap(), [lake]);
        let unread = serde_json::json!({"name": "docs", "version": "1.0.0"});
        let older: InstalledModule = serde_json::from_value(unread).unwrap();
        assert!(older.provides.is_empty(), "an agent that does not say them");
        for bad in [vec!["Lake".to_string()], vec!["x".to_string(); 17]] {
            let module = InstalledModule {
                provides: bad,
                ..module("lake-rustfs", None)
            };
            let refused = service.set_modules(vec![module]).await;
            assert!(
                matches!(refused, Err(UpdatesError::Invalid(..))),
                "{refused:?}"
            );
        }
    }

    #[tokio::test]
    async fn the_whole_list_replaces_the_one_before() {
        let service = UpdatesService::new(Arc::new(MemoryMaintenance::default()));
        assert!(service.installed_modules().await.unwrap().is_empty());
        let first = vec![
            module("quality-data", None),
            module("orchestrator", Some("/internal/drain")),
        ];
        service.set_modules(first.clone()).await.unwrap();
        service.set_modules(first.clone()).await.unwrap();
        assert_eq!(service.installed_modules().await.unwrap(), first);
        service
            .set_modules(vec![module("quality-data", None)])
            .await
            .unwrap();
        assert_eq!(
            service.installed_modules().await.unwrap(),
            [module("quality-data", None)]
        );
    }

    #[tokio::test]
    async fn a_bad_module_is_refused_and_nothing_changes() {
        let service = UpdatesService::new(Arc::new(MemoryMaintenance::default()));
        let kept = vec![module("orchestrator", Some("/drain"))];
        service.set_modules(kept.clone()).await.unwrap();
        let mut no_version = module("grafana", None);
        no_version.version = " ".into();
        for bad in [
            vec![module("Orchestrator", None)],
            vec![module("", None)],
            vec![module("a/b", None)],
            vec![module("orchestrator", None), module("orchestrator", None)],
            vec![no_version],
            vec![module("orchestrator", Some("drain"))],
            vec![module("orchestrator", Some("/"))],
            vec![module("orchestrator", Some("/../api/drain"))],
            vec![module("orchestrator", Some("/drain?x=1"))],
            vec![module("orchestrator", Some("//other/drain"))],
        ] {
            let refused = service.set_modules(bad.clone()).await;
            assert!(
                matches!(refused, Err(UpdatesError::Invalid("modules", _))),
                "{bad:?}"
            );
        }
        assert_eq!(service.installed_modules().await.unwrap(), kept);
    }

    #[tokio::test]
    async fn a_removed_modules_permissions_are_kept() {
        let service = UpdatesService::new(Arc::new(MemoryMaintenance::default()));
        let mut bi = module("bi-superset", None);
        bi.permissions = Some(ModulePermissions::default());
        service.set_modules(vec![bi]).await.unwrap();
        let declared = service.declared_modules().await.unwrap();
        assert_eq!(
            (declared[0].name.as_str(), declared[0].installed),
            ("bi-superset", true)
        );
        service.set_modules(vec![]).await.unwrap();
        let declared = service.declared_modules().await.unwrap();
        assert_eq!(declared.len(), 1);
        assert!(!declared[0].installed, "removed, kept");
    }
}
