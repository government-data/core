//! The secrets a module's machine role declares (quality-sdk decision
//! 0013, the user's ruling 44 of 2026-11-25): the update agent hands their
//! values to the core (`PUT /updater/v1/modules/{module}/role-secrets`),
//! and the core passes them only over mTLS to approved machines of that
//! role, with the registry token. **In memory only**, as the token: never
//! in the database, a file, a log or the audit; replaced by each call for
//! the module; the agent hands them again after telling the modules anew
//! (the API restarted).

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use super::{UpdatesError, UpdatesService};

/// The most secrets a role declares.
const MAX_SECRETS: usize = 32;
/// The longest value taken (a key file).
const MAX_VALUE: usize = 64 * 1024;

/// A role's secrets by name; `Debug` names them, never their values.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct RoleSecrets(pub BTreeMap<String, String>);

impl std::fmt::Debug for RoleSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.0.keys()).finish()
    }
}

/// Every module's roles' secrets: module → role → secrets.
type Map = BTreeMap<String, BTreeMap<String, RoleSecrets>>;

/// The secrets held, shared by every clone of the service.
#[derive(Default, Clone)]
pub(super) struct Held(Arc<RwLock<Map>>);

impl UpdatesService {
    /// Hold `roles`' secrets of the installed `module` in place of the ones
    /// before; each role must be one the module declares. An invalid call
    /// is refused and the ones held are kept.
    pub async fn hold_role_secrets(
        &self,
        module: &str,
        roles: BTreeMap<String, RoleSecrets>,
    ) -> Result<(), UpdatesError> {
        let installed = self
            .store
            .modules()
            .await
            .map_err(UpdatesError::Unavailable)?;
        let Some(declared) = installed.iter().find(|m| m.name == module) else {
            let why = texts::tr!("app.updates.roleSecrets.notInstalled", name = module);
            return Err(UpdatesError::Invalid("module", why));
        };
        for (role, secrets) in &roles {
            if !declared.roles.iter().any(|r| &r.name == role) {
                let why = texts::tr!(
                    "app.updates.roleSecrets.noSuchRole",
                    name = module,
                    role = role
                );
                return Err(UpdatesError::Invalid("roles", why));
            }
            if !valid(secrets) {
                let why = texts::tr!("app.updates.roleSecrets.bad", name = module, role = role);
                return Err(UpdatesError::Invalid("roles", why));
            }
        }
        let mut held = self
            .role_secrets
            .0
            .write()
            .unwrap_or_else(|p| p.into_inner());
        held.insert(module.to_string(), roles);
        Ok(())
    }

    /// The secrets held for `module`'s `role`; none when none was handed.
    pub fn role_secrets(&self, module: &str, role: &str) -> RoleSecrets {
        let held = self
            .role_secrets
            .0
            .read()
            .unwrap_or_else(|p| p.into_inner());
        held.get(module)
            .and_then(|roles| roles.get(role))
            .cloned()
            .unwrap_or_default()
    }
}

/// At most [`MAX_SECRETS`], each named `[A-Za-z0-9._-]` (at most 64) with
/// a value of at most [`MAX_VALUE`] bytes and no NUL.
fn valid(secrets: &RoleSecrets) -> bool {
    secrets.0.len() <= MAX_SECRETS
        && secrets.0.iter().all(|(name, value)| {
            (1..=64).contains(&name.len())
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
                && value.len() <= MAX_VALUE
                && !value.contains('\0')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updates::module_roles::tests::role;
    use crate::updates::tests::MemoryMaintenance;
    use crate::updates::InstalledModule;

    async fn service() -> UpdatesService {
        let service = UpdatesService::new(Arc::new(MemoryMaintenance::default()));
        let mut module: InstalledModule = serde_json::from_value(serde_json::json!({
            "name": "engine-trino", "version": "1.0.0"
        }))
        .unwrap();
        module.roles = vec![role("trino-worker")];
        service.set_modules(vec![module]).await.unwrap();
        service
    }

    fn secrets(pairs: &[(&str, &str)]) -> RoleSecrets {
        RoleSecrets(
            pairs
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        )
    }

    #[tokio::test]
    async fn a_declared_roles_secrets_are_held_and_replaced() {
        let s = service().await;
        let first = secrets(&[("trino-shared-secret", "one"), ("lake-secret-key", "k")]);
        s.hold_role_secrets(
            "engine-trino",
            [("trino-worker".into(), first.clone())].into(),
        )
        .await
        .unwrap();
        assert_eq!(
            s.clone().role_secrets("engine-trino", "trino-worker"),
            first
        );
        let next = secrets(&[("trino-shared-secret", "two")]);
        s.hold_role_secrets(
            "engine-trino",
            [("trino-worker".into(), next.clone())].into(),
        )
        .await
        .unwrap();
        assert_eq!(s.role_secrets("engine-trino", "trino-worker"), next);
        assert_eq!(
            s.role_secrets("engine-trino", "lake"),
            RoleSecrets::default()
        );
    }

    #[tokio::test]
    async fn an_undeclared_role_another_module_or_a_bad_secret_is_refused() {
        let s = service().await;
        let held = secrets(&[("a", "1")]);
        s.hold_role_secrets(
            "engine-trino",
            [("trino-worker".into(), held.clone())].into(),
        )
        .await
        .unwrap();
        let bad_name = secrets(&[("a b", "1")]);
        let nul = secrets(&[("a", "x\0y")]);
        for (module, role, sec) in [
            ("engine-trino", "lake", held.clone()),
            ("docs", "trino-worker", held.clone()),
            ("engine-trino", "trino-worker", bad_name),
            ("engine-trino", "trino-worker", nul),
        ] {
            let refused = s
                .hold_role_secrets(module, [(role.into(), sec)].into())
                .await;
            assert!(
                matches!(refused, Err(UpdatesError::Invalid(..))),
                "{module} {role}"
            );
        }
        assert_eq!(s.role_secrets("engine-trino", "trino-worker"), held, "kept");
    }

    #[test]
    fn the_debug_names_the_secrets_never_their_values() {
        let shown = format!("{:?}", secrets(&[("trino-shared-secret", "hunter2")]));
        assert!(shown.contains("trino-shared-secret") && !shown.contains("hunter2"));
    }
}
