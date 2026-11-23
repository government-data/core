//! Who can do what (ADR 0016): the actor of a session, and managing the IAM
//! records: custom policies, groups, what is attached to each account, and
//! service accounts.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use domain::Role;
use iam::catalog::{MANAGE_POLICIES, MANAGE_USERS};
use iam::{managed, Actor, Permissions, Policy, Statement};
use platform::{Group, GroupFields, NewServiceAccount, PrincipalKind, ServiceAccount, UserAccess};
use serde::Serialize;

use super::{require, PlatformError, PlatformService, R};

/// A policy as Settings shows it: managed or custom, and how many principals
/// have it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyView {
    #[serde(flatten)]
    pub policy: Policy,
    pub attached: i64,
}

/// A group to create or change: its fields, its people added by hand and its
/// policies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupChange {
    pub name: String,
    pub description: String,
    pub directory_groups: Vec<String>,
    pub members: Vec<String>,
    pub policies: Vec<String>,
}

/// A service account just created, with its token (shown once).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreatedServiceAccount {
    pub id: String,
    pub access_key: String,
    pub token: String,
}

fn invalid(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::Invalid(e.to_string())
}

impl PlatformService {
    // --- The actor -----------------------------------------------------------

    /// A session's actor (with `service_account`, of a token), from memory
    /// while still valid: it is reloaded when the permissions change
    /// or after [`crate::actors::TTL`].
    pub async fn actor(
        &self,
        admin_id: &str,
        role: &Role,
        service_account: Option<&str>,
    ) -> R<Actor> {
        let key = match service_account {
            Some(sa) => format!("{admin_id}/{sa}"),
            None => admin_id.to_string(),
        };
        if let Some(actor) = self.actors.get(&key, role.as_str()) {
            return Ok(actor);
        }
        let generation = self.actors.generation();
        let actor = self.load_actor(admin_id, role, service_account).await?;
        self.actors
            .put(&key, role.as_str(), actor.clone(), generation);
        Ok(actor)
    }

    /// What someone can do changed: called by Postgres's `permissions`
    /// notice (changes made in another replica or directly in the database).
    pub fn invalidate_permissions(&self) {
        self.actors.invalidate();
    }

    /// A permission change made here: make it count in this process right
    /// away, without waiting for the notice.
    fn permissions_changed(&self) {
        self.actors.invalidate();
    }

    /// The policies named `names`: managed ones from the code, custom ones
    /// from storage. Unknown names (a removed policy) grant nothing.
    async fn resolve(&self, names: &[String]) -> R<Permissions> {
        if names.is_empty() {
            return Ok(Permissions::default());
        }
        let custom = self.repo.list_policies().await?;
        let policies: Vec<Policy> = names
            .iter()
            .filter_map(|n| {
                managed::find(n).or_else(|| custom.iter().find(|p| &p.name == n).cloned())
            })
            .collect();
        Ok(Permissions::from_policies(&policies))
    }

    async fn load_actor(
        &self,
        admin_id: &str,
        role: &Role,
        service_account: Option<&str>,
    ) -> R<Actor> {
        let is_admin = *role == Role::admin();
        let permissions = if is_admin {
            Permissions::default()
        } else {
            self.resolve(&self.repo.account_policies(admin_id).await?)
                .await?
        };
        let mut actor = Actor::account(admin_id, is_admin, permissions);
        if let Some(sa) = service_account {
            actor.service_account = Some(sa.to_string());
            // Without policies of its own it has its owner's; with some, only
            // what both allow.
            let own = self
                .repo
                .attached(PrincipalKind::ServiceAccount, sa)
                .await?;
            if !own.is_empty() {
                actor.ceiling = Some(self.resolve(&own).await?);
            }
        }
        Ok(actor)
    }

    pub(super) fn require(actor: &Actor, action: &str) -> R<()> {
        if actor.may(action) {
            Ok(())
        } else {
            Err(PlatformError::Forbidden)
        }
    }

    /// That every name is a policy, managed or custom.
    async fn known_policies(&self, names: &[String]) -> R<()> {
        let custom = self.repo.list_policies().await?;
        for n in names {
            if managed::find(n).is_none() && !custom.iter().any(|p| &p.name == n) {
                return Err(PlatformError::Invalid(format!(
                    "policies: {n} does not exist"
                )));
            }
        }
        Ok(())
    }

    // --- Policies ------------------------------------------------------------

    /// The managed policies, then the custom ones, with how many have each.
    pub async fn list_policies(&self, actor: &Actor) -> R<Vec<PolicyView>> {
        if !actor.may(MANAGE_POLICIES) && !actor.may(MANAGE_USERS) {
            return Err(PlatformError::Forbidden);
        }
        let usage: HashMap<String, i64> = self.repo.policy_usage().await?;
        Ok(managed::all()
            .into_iter()
            .chain(self.repo.list_policies().await?)
            .map(|policy| PolicyView {
                attached: usage.get(&policy.name).copied().unwrap_or(0),
                policy,
            })
            .collect())
    }

    /// Create or change a custom policy.
    pub async fn put_policy(
        &self,
        actor: &Actor,
        name: &str,
        description: &str,
        statements: Vec<Statement>,
    ) -> R<()> {
        Self::require(actor, MANAGE_POLICIES)?;
        let name = name.trim();
        iam::policy::validate(name, &statements).map_err(invalid)?;
        let description = description.trim();
        require(
            description.chars().count() <= 500,
            "description",
            "500 characters at most",
        )?;
        self.repo
            .put_policy(&Policy {
                name: name.to_string(),
                description: description.to_string(),
                statements,
                managed: false,
            })
            .await?;
        self.permissions_changed();
        Ok(())
    }

    /// Remove a custom policy; whoever had it loses what it gave.
    pub async fn delete_policy(&self, actor: &Actor, name: &str) -> R<()> {
        Self::require(actor, MANAGE_POLICIES)?;
        if managed::find(name).is_some() {
            return Err(invalid(iam::PolicyError::Reserved));
        }
        self.repo.delete_policy(name).await?;
        self.permissions_changed();
        Ok(())
    }

    // --- Accounts ------------------------------------------------------------

    /// Every account's policies and groups.
    pub async fn user_access(&self, actor: &Actor) -> R<Vec<UserAccess>> {
        if !actor.may(MANAGE_POLICIES) && !actor.may(MANAGE_USERS) {
            return Err(PlatformError::Forbidden);
        }
        Ok(self.repo.user_access().await?)
    }

    /// Replace the policies attached to an account.
    pub async fn set_user_policies(&self, actor: &Actor, id: &str, policies: &[String]) -> R<()> {
        Self::require(actor, MANAGE_POLICIES)?;
        self.known_policies(policies).await?;
        self.repo.attach(PrincipalKind::User, id, policies).await?;
        self.permissions_changed();
        Ok(())
    }

    // --- Groups --------------------------------------------------------------

    pub async fn list_groups(&self, actor: &Actor) -> R<Vec<Group>> {
        if !actor.may(MANAGE_POLICIES) && !actor.may(MANAGE_USERS) {
            return Err(PlatformError::Forbidden);
        }
        Ok(self.repo.list_groups().await?)
    }

    fn group_fields(change: &GroupChange) -> R<GroupFields> {
        let name = change.name.trim().to_string();
        require(
            iam::policy::valid_name(&name),
            "name",
            "lowercase letters, digits and -, up to 64, starting with a letter",
        )?;
        let description = change.description.trim().to_string();
        require(
            description.chars().count() <= 500,
            "description",
            "500 characters at most",
        )?;
        let mut directory_groups: Vec<String> = Vec::new();
        for dn in &change.directory_groups {
            let dn = dn.trim().to_lowercase();
            if dn.is_empty() {
                continue;
            }
            // A group DN has at least one `cn=...,`.
            require(
                dn.contains('=') && dn.contains(','),
                "directory_groups",
                "a full DN, such as CN=Quality,OU=Groups,DC=company,DC=com",
            )?;
            if !directory_groups.contains(&dn) {
                directory_groups.push(dn);
            }
        }
        Ok(GroupFields {
            name,
            description,
            directory_groups,
        })
    }

    /// Create a group with its people and policies; returns its id.
    pub async fn create_group(&self, actor: &Actor, change: &GroupChange) -> R<String> {
        Self::require(actor, MANAGE_POLICIES)?;
        let fields = Self::group_fields(change)?;
        self.known_policies(&change.policies).await?;
        let id = self.repo.create_group(&fields).await?;
        self.repo.set_group_members(&id, &change.members).await?;
        self.repo
            .attach(PrincipalKind::Group, &id, &change.policies)
            .await?;
        self.permissions_changed();
        Ok(id)
    }

    /// Change a group: its fields, its people added by hand and its policies.
    pub async fn update_group(&self, actor: &Actor, id: &str, change: &GroupChange) -> R<()> {
        Self::require(actor, MANAGE_POLICIES)?;
        let fields = Self::group_fields(change)?;
        self.known_policies(&change.policies).await?;
        self.repo.update_group(id, &fields).await?;
        self.repo.set_group_members(id, &change.members).await?;
        self.repo
            .attach(PrincipalKind::Group, id, &change.policies)
            .await?;
        self.permissions_changed();
        Ok(())
    }

    pub async fn delete_group(&self, actor: &Actor, id: &str) -> R<()> {
        Self::require(actor, MANAGE_POLICIES)?;
        self.repo.delete_group(id).await?;
        self.permissions_changed();
        Ok(())
    }

    // --- Service accounts ----------------------------------------------------

    /// One's own, or every one with `admin:ManagePolicies`.
    pub async fn list_service_accounts(&self, actor: &Actor) -> R<Vec<ServiceAccount>> {
        let owner = (!actor.may(MANAGE_POLICIES)).then_some(actor.admin_id.as_str());
        Ok(self.repo.list_service_accounts(owner).await?)
    }

    /// A service account for oneself: its token is shown only now. A service
    /// account cannot make others.
    pub async fn create_service_account(
        &self,
        actor: &Actor,
        name: &str,
        policies: &[String],
        expires_at: Option<DateTime<Utc>>,
    ) -> R<CreatedServiceAccount> {
        if actor.service_account.is_some() {
            return Err(PlatformError::Forbidden);
        }
        let mint = self
            .tokens
            .as_ref()
            .ok_or_else(|| PlatformError::Internal("no token mint".into()))?;
        let name = name.trim();
        require(
            !name.is_empty() && name.chars().count() <= 100,
            "name",
            "between 1 and 100 characters",
        )?;
        if let Some(at) = expires_at {
            require(at > Utc::now(), "expires_at", "it has to be in the future")?;
        }
        self.known_policies(policies).await?;
        let minted = mint.mint();
        let id = self
            .repo
            .create_service_account(&NewServiceAccount {
                owner_id: actor.admin_id.clone(),
                name: name.to_string(),
                access_key: minted.access_key.clone(),
                secret_hash: minted.secret_hash,
                expires_at,
            })
            .await?;
        self.repo
            .attach(PrincipalKind::ServiceAccount, &id, policies)
            .await?;
        Ok(CreatedServiceAccount {
            id,
            access_key: minted.access_key,
            token: minted.token,
        })
    }

    /// Remove one's own, or anyone's with `admin:ManagePolicies`; its token
    /// stops working at once.
    pub async fn delete_service_account(&self, actor: &Actor, id: &str) -> R<()> {
        let owner = (!actor.may(MANAGE_POLICIES)).then_some(actor.admin_id.as_str());
        self.repo.delete_service_account(id, owner).await?;
        self.permissions_changed();
        Ok(())
    }
}
