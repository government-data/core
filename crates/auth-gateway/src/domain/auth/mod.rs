//! Admin authentication domain: the model and ports for logging an
//! administrator in with an email and password.
//!
//! Following the hexagonal discipline of this crate, everything here is pure:
//! value objects ([`Email`], [`PasswordPolicy`], [`PasswordHash`],
//! [`LockoutPolicy`], [`LockoutState`]) and *ports* (traits) the outer layers
//! implement — [`PasswordHasher`], [`AdminRepository`], [`IpLockoutStore`],
//! [`Clock`](crate::domain::Clock). The domain declares *what* a password hasher or a repository must
//! do; the `infrastructure` crate decides *how* (argon2, Postgres). No web
//! framework, no database driver, no clock reaches in here.

use std::time::{Duration, SystemTime};

use crate::domain::{AdminId, Email, RepositoryError, Role};

mod lockout;
mod password;

pub use lockout::*;
pub use password::*;

/// A stored administrator account, as the repository hands it to the domain.
#[derive(Debug, Clone)]
pub struct AdminAccount {
    /// Opaque unique identifier (the database primary key, as text).
    pub id: AdminId,
    /// The account's normalized email.
    pub email: Email,
    /// The stored password hash to verify against.
    pub password_hash: PasswordHash,
    /// The account's current lockout counters.
    pub lockout: LockoutState,
    /// The account's role.
    pub role: Role,
    /// An optional display name for the profile UI.
    pub display_name: Option<String>,
}

/// A new administrator to persist (used by the bootstrap use case).
#[derive(Debug, Clone)]
pub struct NewAdmin {
    /// The account's normalized email.
    pub email: Email,
    /// The already-hashed password.
    pub password_hash: PasswordHash,
    /// The account's role.
    pub role: Role,
}

/// A profile picture: the image as the provider served it (JPEG, PNG, WebP
/// or GIF, small), kept so the UI does not depend on the provider's URL.
#[derive(Clone, PartialEq, Eq)]
pub struct ProfilePicture {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

impl ProfilePicture {
    /// The largest picture kept.
    pub const MAX_BYTES: usize = 256 * 1024;

    /// The image types kept (never SVG: it can carry script).
    pub fn accepts(content_type: &str) -> bool {
        matches!(
            content_type,
            "image/jpeg" | "image/png" | "image/webp" | "image/gif"
        )
    }
}

impl std::fmt::Debug for ProfilePicture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProfilePicture")
            .field("content_type", &self.content_type)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// Port: persistence for administrator accounts.
///
/// Implemented by a Postgres adapter in `infrastructure`. The domain names the
/// operations the login and bootstrap use cases need; the adapter owns the SQL.
#[async_trait::async_trait]
pub trait AdminRepository: Send + Sync {
    /// Find an account by its normalized email, if one exists.
    async fn find_by_email(&self, email: &Email) -> Result<Option<AdminAccount>, RepositoryError>;

    /// Find an account by its opaque id, if one exists.
    async fn find_by_id(&self, id: &AdminId) -> Result<Option<AdminAccount>, RepositoryError>;

    /// Overwrite an account's lockout counters.
    async fn update_lockout(
        &self,
        id: &AdminId,
        state: &LockoutState,
    ) -> Result<(), RepositoryError>;

    /// Number of administrator accounts (used to gate first-admin bootstrap).
    async fn count(&self) -> Result<u64, RepositoryError>;

    /// Insert a new administrator, returning its assigned id.
    ///
    /// Implementations reject a duplicate email with
    /// [`RepositoryError::EmailTaken`] rather than a generic error, so the
    /// bootstrap use case can report it precisely.
    async fn insert(&self, admin: &NewAdmin) -> Result<AdminId, RepositoryError>;

    /// The account's profile picture, if it has one.
    async fn avatar(&self, _id: &AdminId) -> Result<Option<ProfilePicture>, RepositoryError> {
        Ok(None)
    }

    /// When the profile picture last changed (Unix seconds), to version its
    /// URL so browsers cache it until it changes.
    async fn avatar_version(&self, _id: &AdminId) -> Result<Option<i64>, RepositoryError> {
        Ok(None)
    }

    /// Store (or with `None`, drop) the picture the person chose: it wins
    /// over the provider's until removed.
    async fn set_own_avatar(
        &self,
        id: &AdminId,
        picture: Option<&ProfilePicture>,
    ) -> Result<(), RepositoryError> {
        self.set_avatar(id, picture).await
    }

    /// Store (or with `None`, drop) the account's profile picture (the
    /// provider's: not over one the person chose).
    async fn set_avatar(
        &self,
        _id: &AdminId,
        _picture: Option<&ProfilePicture>,
    ) -> Result<(), RepositoryError> {
        Ok(())
    }

    /// The interface language the account chose (`es` / `en`), if any.
    async fn locale(&self, _id: &AdminId) -> Result<Option<String>, RepositoryError> {
        Ok(None)
    }

    /// Remember (or with `None`, forget) the account's interface language.
    async fn set_locale(
        &self,
        _id: &AdminId,
        _locale: Option<&str>,
    ) -> Result<(), RepositoryError> {
        Ok(())
    }

    /// Every account, by email — for the admin's user list.
    async fn list_accounts(&self) -> Result<Vec<AccountSummary>, RepositoryError> {
        Ok(Vec::new())
    }

    /// Create an account that still has to choose its password
    /// ([`PasswordHash::pending_activation`]). A duplicate email is
    /// [`RepositoryError::EmailTaken`].
    async fn create_invited(
        &self,
        _email: &Email,
        _display_name: Option<&str>,
        _role: &Role,
    ) -> Result<AdminId, RepositoryError> {
        Err(RepositoryError::Backend(
            "creating users is not supported".into(),
        ))
    }

    /// Delete an account (its sessions, links and memberships go with it).
    /// `false` when there was none.
    async fn delete_account(&self, _id: &AdminId) -> Result<bool, RepositoryError> {
        Ok(false)
    }

    /// Change an account's display name and role. Its open sessions take the
    /// new role at once (they carry it). `false` when there was none.
    async fn update_account(
        &self,
        _id: &AdminId,
        _display_name: Option<&str>,
        _role: &Role,
    ) -> Result<bool, RepositoryError> {
        Err(RepositoryError::Backend(
            "editing users is not supported".into(),
        ))
    }
}

/// How an account signs in, as the user list shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStatus {
    /// It has a local password.
    Active,
    /// An admin created it and it has not chosen its password yet.
    Pending,
    /// Its password lives in the corporate directory (LDAP / AD).
    Directory,
}

impl AccountStatus {
    /// The status an account with `hash` has.
    pub fn of(hash: &PasswordHash) -> Self {
        if hash.is_pending_activation() {
            Self::Pending
        } else if hash.is_directory_managed() {
            Self::Directory
        } else {
            Self::Active
        }
    }

    /// The stable string form (`active`, `pending`, `directory`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Pending => "pending",
            Self::Directory => "directory",
        }
    }
}

/// One account in the admin's user list.
#[derive(Debug, Clone)]
pub struct AccountSummary {
    pub id: AdminId,
    pub email: Email,
    pub display_name: Option<String>,
    pub role: Role,
    pub status: AccountStatus,
    pub created_at: SystemTime,
}

#[cfg(test)]
mod tests;
