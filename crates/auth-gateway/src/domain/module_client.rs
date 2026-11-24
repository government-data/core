//! What a client registered by a module carries beyond any client
//! (quality-sdk decision 0009): the module it belongs to, whether it is
//! disabled (the module removed), the secret it replaced for at most an hour
//! (a rotation), the claim whose values come from the person's actions, the
//! capabilities its machine client consumes and where a person may land
//! after signing out. A client registered by hand (`server oidc-client`) has
//! none of it, and a module never takes it over.

use std::time::SystemTime;

use crate::domain::{AdminAccount, RepositoryError};

/// The module side of a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleClient {
    /// The module that registered it.
    pub module: String,
    /// No code and no token while it is disabled.
    pub disabled: bool,
    /// The secret it had before the last change, still taken until then.
    pub previous_secret: Option<PreviousSecret>,
    /// Sent in the id_token and at `/userinfo`.
    pub role_claim: Option<RoleClaim>,
    /// The capabilities its machine client consumes (`sql-query`).
    pub consumes: Vec<String>,
    /// Where a person may be sent after signing out.
    pub logout_uris: Vec<String>,
}

impl ModuleClient {
    /// A module's client with nothing else set.
    pub fn of(module: impl Into<String>) -> Self {
        Self {
            module: module.into(),
            disabled: false,
            previous_secret: None,
            role_claim: None,
            consumes: Vec::new(),
            logout_uris: Vec::new(),
        }
    }
}

/// A secret replaced by a rotation: only its hash, and until when it is
/// still taken.
#[derive(Clone, PartialEq, Eq)]
pub struct PreviousSecret {
    pub hash: String,
    pub until: SystemTime,
}

// The hash is not the secret, but it has no business in logs either.
impl std::fmt::Debug for PreviousSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PreviousSecret(<redacted>)")
    }
}

/// A claim whose values are those whose action the person holds now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleClaim {
    /// The claim's name (`bi_roles`).
    pub name: String,
    pub values: Vec<RoleValue>,
}

/// One value of a [`RoleClaim`] and the action that gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleValue {
    pub value: String,
    pub action: String,
}

impl RoleClaim {
    /// The values given by the actions in `held`, in their order.
    pub fn values_for(&self, held: &[String]) -> Vec<String> {
        self.values
            .iter()
            .filter(|v| held.contains(&v.action))
            .map(|v| v.value.clone())
            .collect()
    }

    /// The actions its values name, each once.
    pub fn actions(&self) -> Vec<String> {
        let mut actions: Vec<String> = Vec::new();
        for v in &self.values {
            if !actions.contains(&v.action) {
                actions.push(v.action.clone());
            }
        }
        actions
    }
}

/// Port: which actions a person holds on some resource (IAM's `can_some`),
/// for a [`RoleClaim`].
#[async_trait::async_trait]
pub trait ActionHolder: Send + Sync {
    /// Of `actions`, those `admin` holds on some resource.
    async fn held(
        &self,
        admin: &AdminAccount,
        actions: &[String],
    ) -> Result<Vec<String>, RepositoryError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_values_of_the_actions_held() {
        let value = |value: &str, action: &str| RoleValue {
            value: value.into(),
            action: action.into(),
        };
        let claim = RoleClaim {
            name: "bi_roles".into(),
            values: vec![
                value("viewer", "quality:ViewResults"),
                value("author", "bi-superset:Author"),
                value("editor", "bi-superset:Author"),
            ],
        };
        assert_eq!(
            claim.actions(),
            ["quality:ViewResults", "bi-superset:Author"]
        );
        let held = ["bi-superset:Author".to_string()];
        assert_eq!(claim.values_for(&held), ["author", "editor"]);
        assert!(claim.values_for(&[]).is_empty());
    }
}
