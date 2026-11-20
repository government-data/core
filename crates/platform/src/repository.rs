//! The persistence port of the platform.

use std::collections::HashMap;

use iam::Policy;

use crate::iam::{
    Group, GroupFields, NewServiceAccount, PrincipalKind, ServiceAccount, UserAccess,
};
use crate::records::{NewSecret, Secret, SecretId};

/// A storage failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryError {
    /// It does not exist.
    NotFound,
    /// It breaks a uniqueness or a reference (a repeated name).
    Conflict(String),
    /// Any other backend failure.
    Backend(String),
}

impl std::fmt::Display for RepositoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("not found"),
            Self::Conflict(m) => write!(f, "conflict: {m}"),
            Self::Backend(m) => write!(f, "storage error: {m}"),
        }
    }
}

impl std::error::Error for RepositoryError {}

type R<T> = Result<T, RepositoryError>;

/// Persistence of secrets, IAM and the license usage.
#[async_trait::async_trait]
pub trait PlatformRepository: Send + Sync {
    // --- Global secrets ----------------------------------------------------

    /// Those whose names `scope` allows, without the values.
    async fn list_secrets(&self, scope: &iam::Scope) -> R<Vec<Secret>>;
    /// `Conflict` if there is already one with that name.
    async fn create_secret(&self, secret: &NewSecret) -> R<Secret>;
    /// Change the description and, if given, the value (already encrypted)
    /// with its public part (of a connection).
    async fn update_secret(
        &self,
        id: &SecretId,
        description: Option<&str>,
        sealed: Option<(&str, &serde_json::Value)>,
    ) -> R<Secret>;
    async fn delete_secret(&self, id: &SecretId) -> R<()>;
    /// The encrypted values of the ones that exist, by id.
    async fn secret_values(&self, ids: &[SecretId]) -> R<HashMap<SecretId, String>>;

    // --- IAM: policies (ADR 0016) -------------------------------------------

    /// The custom policies (the managed ones live in `iam::managed`).
    async fn list_policies(&self) -> R<Vec<Policy>>;
    /// Create or replace a custom policy.
    async fn put_policy(&self, policy: &Policy) -> R<()>;
    /// Remove a custom policy and its attachments.
    async fn delete_policy(&self, name: &str) -> R<()>;
    /// How many principals each policy is attached to, by name.
    async fn policy_usage(&self) -> R<HashMap<String, i64>>;
    /// The policies attached to a principal.
    async fn attached(&self, kind: PrincipalKind, id: &str) -> R<Vec<String>>;
    /// Replace the policies attached to a principal (`NotFound` if it does
    /// not exist).
    async fn attach(&self, kind: PrincipalKind, id: &str, policies: &[String]) -> R<()>;
    /// The names of an account's policies: its own and its groups'.
    async fn account_policies(&self, admin_id: &str) -> R<Vec<String>>;
    /// Every account's policies and groups.
    async fn user_access(&self) -> R<Vec<UserAccess>>;

    // --- IAM: groups ---------------------------------------------------------

    async fn list_groups(&self) -> R<Vec<Group>>;
    /// `Conflict` if the name is taken.
    async fn create_group(&self, fields: &GroupFields) -> R<String>;
    async fn update_group(&self, id: &str, fields: &GroupFields) -> R<()>;
    async fn delete_group(&self, id: &str) -> R<()>;
    /// Replace the members added by hand (those from the directory stay).
    async fn set_group_members(&self, id: &str, admin_ids: &[String]) -> R<()>;

    // --- IAM: service accounts -----------------------------------------------

    /// Those of `owner`, or all of them.
    async fn list_service_accounts(&self, owner: Option<&str>) -> R<Vec<ServiceAccount>>;
    /// `Conflict` if the owner already has one with that name.
    async fn create_service_account(&self, account: &NewServiceAccount) -> R<String>;
    /// Remove it, if it is `owner`'s (or anyone's, with `None`).
    async fn delete_service_account(&self, id: &str, owner: Option<&str>) -> R<()>;

    // --- Notices -----------------------------------------------------------

    /// The administrators' emails (the update notices).
    async fn admin_emails(&self) -> R<Vec<String>> {
        Ok(Vec::new())
    }
}
