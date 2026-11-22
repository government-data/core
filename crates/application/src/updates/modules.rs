//! The installed modules, as the update agent tells them (decision 0003):
//! the whole list each time (`PUT /updater/v1/modules`), each with its
//! version and, when its manifest declares one, its drain path. The drain
//! asks every module that has a drain path ([`UpdatesService::installed_modules`]).

use serde::{Deserialize, Serialize};

use super::{UpdatesError, UpdatesService};

/// The longest name, version or drain path taken.
const MAX_LEN: usize = 200;

/// An installed module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledModule {
    /// Its folder's name: lower-case letters, digits and `-`.
    pub name: String,
    pub version: String,
    /// Where it answers its drain (`/…`), `None` when it declares none.
    #[serde(default)]
    pub drain_path: Option<String>,
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
    }
    Ok(())
}

fn is_name(name: &str) -> bool {
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
            vec![module("trino", None), module("trino", None)],
            vec![no_version],
            vec![module("trino", Some("drain"))],
            vec![module("trino", Some("/"))],
            vec![module("trino", Some("/../api/drain"))],
            vec![module("trino", Some("/drain?x=1"))],
            vec![module("trino", Some("//other/drain"))],
        ] {
            let refused = service.set_modules(bad.clone()).await;
            assert!(
                matches!(refused, Err(UpdatesError::Invalid("modules", _))),
                "{bad:?}"
            );
        }
        assert_eq!(service.installed_modules().await.unwrap(), kept);
    }
}
