//! Ports for signing in against a corporate directory (LDAP / Active
//! Directory).
//!
//! A directory account never has a local password: the directory verifies the
//! credentials ([`DirectoryAuthenticator`]), and the local `admin_users` row is
//! created or refreshed on each successful sign-in ([`DirectoryAccounts`]) with
//! the [`PasswordHash::directory_managed`] marker in place of a hash. Local
//! accounts (the bootstrap admin, a break-glass account) keep working with
//! argon2 alongside.
//!
//! [`PasswordHash::directory_managed`]: crate::domain::PasswordHash::directory_managed

use crate::domain::{AdminAccount, AdminId, Email, RepositoryError, Role};

/// What the directory says about a person who just proved their password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryUser {
    /// The directory's login name (`sAMAccountName` or `uid`).
    pub username: String,
    /// The person's email, used as the local account key.
    pub email: Email,
    /// Display name (`displayName` / `cn`), if the directory has one.
    pub display_name: Option<String>,
    /// Full DNs of the groups the person belongs to (`memberOf`).
    pub groups: Vec<String>,
}

/// Why the directory could not answer (distinct from "wrong password").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryError(pub String);

impl std::fmt::Display for DirectoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "directory unavailable: {}", self.0)
    }
}

impl std::error::Error for DirectoryError {}

/// Port: verify credentials against the directory.
#[async_trait::async_trait]
pub trait DirectoryAuthenticator: Send + Sync {
    /// `Ok(Some(user))` when the password is right, `Ok(None)` when the
    /// account does not exist or the password is wrong (indistinguishable on
    /// purpose), `Err` when the directory could not be reached or queried.
    async fn authenticate(
        &self,
        login: &str,
        password: &str,
    ) -> Result<Option<DirectoryUser>, DirectoryError>;
}

/// Port: the local side of a directory sign-in.
#[async_trait::async_trait]
pub trait DirectoryAccounts: Send + Sync {
    /// Whether any of `groups` grants access to anything in this application.
    /// A directory user with none of them is refused, so an employee who was
    /// never given access cannot even open an empty session.
    async fn grants_access(&self, groups: &[String]) -> Result<bool, RepositoryError>;

    /// Create the local account for `user`, or refresh its email, display name
    /// and role, returning it.
    async fn upsert(
        &self,
        user: &DirectoryUser,
        role: &Role,
    ) -> Result<AdminAccount, RepositoryError>;

    /// Re-derive the account's group-based permissions from `groups`.
    async fn sync_groups(&self, id: &AdminId, groups: &[String]) -> Result<(), RepositoryError>;
}
