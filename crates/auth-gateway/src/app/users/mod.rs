//! Users, as an admin manages them from Settings: the list, creating an
//! account (which gets an activation link by email to choose its password),
//! sending that link again, changing its name or role, and deleting it.
//!
//! A created account has no password ([`PasswordHash::pending_activation`]),
//! so nobody can sign in to it until its owner opens the link — or, with
//! OAuth, signs in with a provider that vouches for the same email.

use std::sync::Arc;

use crate::{AccountSummary, AdminAccount, AdminRepository, PasswordHash};
use domain::{AdminId, Email, RepositoryError, Role};

use crate::app::auth::DIRECTORY_USER_ROLE;
use crate::app::password_reset::{PasswordResetService, ResetError};

/// The roles an account can have: `admin` (everything), or `user` (whose
/// permissions come from its policies and its groups', ADR 0016).
pub const ASSIGNABLE_ROLES: [&str; 2] = ["admin", DIRECTORY_USER_ROLE];

/// Why a user operation failed.
#[derive(Debug)]
pub enum UsersError {
    /// A field is invalid; `(field, reason)`.
    Invalid(&'static str, String),
    /// Another account already has that email.
    EmailTaken,
    /// No such account.
    NotFound,
    /// The account already chose its password (or signs in with the
    /// directory), so there is nothing to activate.
    NotPending,
    /// An admin cannot delete their own account.
    SelfDelete,
    /// Deleting it would leave nobody with the `admin` role.
    LastAdmin,
    /// An admin cannot change their own role (they would lock themselves
    /// out of Settings).
    SelfRole,
    /// Only an `admin` can give or take the `admin` role, or change an
    /// `admin`'s account.
    AdminOnly,
    /// The account signs in with the directory, which sets its name and role
    /// on every sign-in.
    DirectoryManaged,
    /// No outgoing mail is configured, so no activation link can be sent.
    MailUnavailable,
    /// The link could not be sent (the account was created anyway).
    MailFailed(String),
    /// A storage failure; the detail is for the log.
    Internal(String),
}

impl std::fmt::Display for UsersError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(field, reason) => write!(f, "{field}: {reason}"),
            Self::EmailTaken => f.write_str("another account already has that email"),
            Self::NotFound => f.write_str("no such account"),
            Self::NotPending => f.write_str("the account is already active"),
            Self::SelfDelete => f.write_str("an admin cannot delete their own account"),
            Self::LastAdmin => f.write_str("it is the only admin"),
            Self::SelfRole => f.write_str("an admin cannot change their own role"),
            Self::AdminOnly => f.write_str("only an admin can do that to an admin"),
            Self::DirectoryManaged => f.write_str("the directory manages that account"),
            Self::MailUnavailable => f.write_str("outgoing mail is not configured"),
            Self::MailFailed(e) => write!(f, "the activation email could not be sent: {e}"),
            Self::Internal(e) => write!(f, "internal error: {e}"),
        }
    }
}

impl std::error::Error for UsersError {}

/// The display name trimmed (`None` when blank), at most 120 characters.
fn clean_name(raw: Option<&str>) -> Result<Option<&str>, UsersError> {
    let name = raw.map(str::trim).filter(|n| !n.is_empty());
    if name.is_some_and(|n| n.chars().count() > 120) {
        return Err(UsersError::Invalid("display_name", "too long".into()));
    }
    Ok(name)
}

fn internal(e: impl std::fmt::Display) -> UsersError {
    UsersError::Internal(e.to_string())
}

fn mail_error(e: ResetError) -> UsersError {
    match e {
        ResetError::MailUnavailable => UsersError::MailUnavailable,
        ResetError::Internal(e) => UsersError::MailFailed(e),
        other => UsersError::Internal(other.to_string()),
    }
}

/// A new account, as the admin typed it.
#[derive(Debug, Clone)]
pub struct NewUser {
    pub email: String,
    pub display_name: Option<String>,
    pub role: String,
}

/// The new name and role of an account, as the admin typed them.
#[derive(Debug, Clone)]
pub struct UserChanges {
    pub display_name: Option<String>,
    pub role: String,
}

/// What [`UsersService::create`] did.
#[derive(Debug, Clone)]
pub struct CreatedUser {
    pub id: AdminId,
    /// Whether the activation email left.
    pub activation_sent: bool,
}

/// Application service for managing users.
#[derive(Clone)]
pub struct UsersService {
    admins: Arc<dyn AdminRepository>,
    resets: PasswordResetService,
}

impl std::fmt::Debug for UsersService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsersService").finish_non_exhaustive()
    }
}

impl UsersService {
    /// Assemble the service: the accounts, and the recovery service that
    /// issues and emails the activation links.
    pub fn new(admins: Arc<dyn AdminRepository>, resets: PasswordResetService) -> Self {
        Self { admins, resets }
    }

    /// Every account.
    pub async fn list(&self) -> Result<Vec<AccountSummary>, UsersError> {
        self.admins.list_accounts().await.map_err(internal)
    }

    /// The roles an account can have: `admin` and `user`.
    pub async fn roles(&self) -> Result<Vec<String>, UsersError> {
        Ok(ASSIGNABLE_ROLES.iter().map(|r| r.to_string()).collect())
    }

    /// Whoever manages users without being `admin` (by a policy with
    /// `admin:ManageUsers`) cannot make anyone `admin` nor touch an `admin`'s
    /// account: it would let them climb above their own policies.
    pub async fn guard_admin(
        &self,
        actor_is_admin: bool,
        target: Option<&AdminId>,
        new_role: Option<&str>,
    ) -> Result<(), UsersError> {
        if actor_is_admin {
            return Ok(());
        }
        let admin = Role::admin();
        // Compared as `assignable_role` will store it: `Admin` or ` ADMIN `
        // become `admin` once parsed.
        if new_role.is_some_and(|r| Role::parse(r).is_ok_and(|r| r == admin)) {
            return Err(UsersError::AdminOnly);
        }
        if let Some(id) = target {
            let account = self.admins.find_by_id(id).await.map_err(internal)?;
            if account.is_some_and(|a| a.role == admin) {
                return Err(UsersError::AdminOnly);
            }
        }
        Ok(())
    }

    /// One of [`Self::roles`], or [`UsersError::Invalid`].
    async fn assignable_role(&self, raw: &str) -> Result<Role, UsersError> {
        let role =
            Role::parse(raw).map_err(|_| UsersError::Invalid("role", "unknown role".into()))?;
        if self.roles().await?.iter().any(|r| r == role.as_str()) {
            Ok(role)
        } else {
            Err(UsersError::Invalid("role", "unknown role".into()))
        }
    }

    /// Create an account and email it the activation link. Nothing is
    /// created without outgoing mail; if the email then fails, the account
    /// stays (pending), `activation_sent` says so, and the admin sends it
    /// again from the list.
    pub async fn create(&self, new: NewUser, link_base: &str) -> Result<CreatedUser, UsersError> {
        let email =
            Email::parse(&new.email).map_err(|e| UsersError::Invalid("email", e.to_string()))?;
        let role = self.assignable_role(&new.role).await?;
        let display_name = clean_name(new.display_name.as_deref())?;
        if !self.resets.mail_configured() {
            return Err(UsersError::MailUnavailable);
        }

        let id = self
            .admins
            .create_invited(&email, display_name, &role)
            .await
            .map_err(|e| match e {
                RepositoryError::EmailTaken => UsersError::EmailTaken,
                other => internal(other),
            })?;
        let account = AdminAccount {
            id: id.clone(),
            email,
            password_hash: PasswordHash::pending_activation(),
            lockout: crate::LockoutState::clear(),
            role,
            display_name: display_name.map(str::to_string),
        };
        let activation_sent = match self.resets.send_activation(&account, link_base).await {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(admin = %account.id, "users: activation email failed: {e}");
                false
            }
        };
        Ok(CreatedUser {
            id,
            activation_sent,
        })
    }

    /// Email a pending account its activation link again (the earlier link
    /// stops working). Returns the account, for the audit trail.
    pub async fn resend(&self, id: &AdminId, link_base: &str) -> Result<AdminAccount, UsersError> {
        let account = self
            .admins
            .find_by_id(id)
            .await
            .map_err(internal)?
            .ok_or(UsersError::NotFound)?;
        if !account.password_hash.is_pending_activation() {
            return Err(UsersError::NotPending);
        }
        self.resets
            .send_activation(&account, link_base)
            .await
            .map_err(mail_error)?;
        Ok(account)
    }

    /// Change an account's display name and role. `actor` (the admin doing
    /// it) cannot change their own role, the last `admin` cannot stop being
    /// one, and a directory account keeps what the directory says. Returns
    /// the account's email, for the audit trail.
    pub async fn update(
        &self,
        actor: &AdminId,
        id: &AdminId,
        changes: UserChanges,
    ) -> Result<Email, UsersError> {
        let role = self.assignable_role(&changes.role).await?;
        let display_name = clean_name(changes.display_name.as_deref())?;
        let accounts = self.list().await?;
        let target = accounts
            .iter()
            .find(|a| &a.id == id)
            .ok_or(UsersError::NotFound)?;
        let unchanged_role = target.role == role;
        if target.status == crate::AccountStatus::Directory
            && (!unchanged_role || target.display_name.as_deref() != display_name)
        {
            return Err(UsersError::DirectoryManaged);
        }
        if !unchanged_role {
            if actor == id {
                return Err(UsersError::SelfRole);
            }
            let admin = Role::admin();
            let admins = accounts.iter().filter(|a| a.role == admin).count();
            if target.role == admin && admins <= 1 {
                return Err(UsersError::LastAdmin);
            }
        }
        if !self
            .admins
            .update_account(id, display_name, &role)
            .await
            .map_err(internal)?
        {
            return Err(UsersError::NotFound);
        }
        Ok(target.email.clone())
    }

    /// Delete an account. `actor` (the admin doing it) cannot delete
    /// themselves, and the last `admin` cannot go. Returns the deleted
    /// account's email, for the audit trail.
    pub async fn delete(&self, actor: &AdminId, id: &AdminId) -> Result<Email, UsersError> {
        if actor == id {
            return Err(UsersError::SelfDelete);
        }
        let accounts = self.list().await?;
        let target = accounts
            .iter()
            .find(|a| &a.id == id)
            .ok_or(UsersError::NotFound)?;
        let admin = Role::admin();
        let admins = accounts.iter().filter(|a| a.role == admin).count();
        if target.role == admin && admins <= 1 {
            return Err(UsersError::LastAdmin);
        }
        let email = target.email.clone();
        // To notify it, before it stops existing.
        let account = self.admins.find_by_id(id).await.map_err(internal)?;
        let lang = self.resets.language_of(id).await;
        if !self.admins.delete_account(id).await.map_err(internal)? {
            return Err(UsersError::NotFound);
        }
        if let Some(account) = account {
            self.resets.notify_deleted(&account, lang).await;
        }
        Ok(email)
    }
}

#[cfg(test)]
mod tests;
