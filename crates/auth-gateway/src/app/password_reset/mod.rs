//! "Forgot your password?": a reset link by email, with an expiry.
//!
//! [`PasswordResetService::request`] answers the same way whether or not the
//! email has an account, and sends the mail in the background so the response
//! time does not tell either. Only local accounts get a link: a directory
//! (LDAP / AD) account's password lives in the directory, so its owner gets a
//! note saying so instead. Accounts that sign in only through OAuth keep their
//! password with the provider, which has its own recovery; this flow never
//! touches them unless they also have a local password.
//!
//! [`PasswordResetService::reset`] enforces the password policy, then redeems
//! the link and sets the new password in one step, which also clears the
//! account's lockout and ends its sessions everywhere.

use std::sync::Arc;
use std::time::Duration;

use crate::{
    AdminAccount, AdminRepository, NewAdmin, PasswordHash, PasswordHasher, PasswordPolicy,
    PasswordPolicyError, PasswordResetStore, ResetTokenGenerator,
};
use domain::{AdminId, Clock, Email, Role};

use crate::app::account_mail::{
    activation_mail, deleted_mail, directory_mail, language, password_changed_mail, reset_mail,
    welcome_mail,
};
use crate::app::mail::MailSlot;

/// The path of the web form the link opens; the token travels in the fragment
/// (`#…`), so it never reaches a server log or a `Referer` header.
pub const RESET_FORM_PATH: &str = "/reset-password";

/// The form an activation link opens: the same token and endpoints as a
/// reset, with a welcome instead of "forgot your password?".
pub const ACTIVATION_FORM_PATH: &str = "/activate";

/// How long an activation link works: the person may not read the email the
/// same day, so longer than a reset link.
pub const ACTIVATION_TTL: Duration = Duration::from_secs(72 * 60 * 60);

/// Why a reset step failed.
#[derive(Debug)]
pub enum ResetError {
    /// No outgoing mail is configured, so no link can be sent.
    MailUnavailable,
    /// The link is unknown, already used, replaced by a newer one or expired.
    InvalidToken,
    /// The new password does not meet the policy.
    WeakPassword(PasswordPolicyError),
    /// A hashing or storage failure; the detail is for the log.
    Internal(String),
}

impl std::fmt::Display for ResetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MailUnavailable => f.write_str("outgoing mail is not configured"),
            Self::InvalidToken => f.write_str("invalid or expired reset link"),
            Self::WeakPassword(e) => write!(f, "{e}"),
            Self::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for ResetError {}

fn internal(e: impl std::fmt::Display) -> ResetError {
    ResetError::Internal(e.to_string())
}

/// What [`PasswordResetService::admin_access`] left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminAccess {
    pub admin: AdminId,
    /// There was no account and this one was created, pending.
    pub created: bool,
    /// The link to choose the password; `None` if the account already has one
    /// and `force` was not asked.
    pub link: Option<String>,
    /// The link also went out by email.
    pub emailed: bool,
}

/// Why the administrator's entry was not given.
#[derive(Debug)]
pub enum AccessError {
    InvalidEmail(String),
    /// There are accounts already and none with that email.
    UnknownAccount,
    /// That account's password is in the directory (LDAP / AD).
    DirectoryAccount,
    Internal(String),
}

impl std::fmt::Display for AccessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEmail(e) => write!(f, "invalid email: {e}"),
            Self::UnknownAccount => f.write_str("there are accounts and none has that email"),
            Self::DirectoryAccount => f.write_str("that account's password lives in the directory"),
            Self::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for AccessError {}

fn access_internal(e: impl std::fmt::Display) -> AccessError {
    AccessError::Internal(e.to_string())
}

/// Application service for password recovery.
#[derive(Clone)]
pub struct PasswordResetService {
    admins: Arc<dyn AdminRepository>,
    store: Arc<dyn PasswordResetStore>,
    tokens: Arc<dyn ResetTokenGenerator>,
    hasher: Arc<dyn PasswordHasher>,
    clock: Arc<dyn Clock>,
    mail: MailSlot,
    policy: PasswordPolicy,
    ttl: Duration,
}

impl std::fmt::Debug for PasswordResetService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PasswordResetService")
            .field("policy", &self.policy)
            .field("ttl", &self.ttl)
            .finish_non_exhaustive()
    }
}

impl PasswordResetService {
    /// Assemble the service. `ttl` is how long a link works unless the mail
    /// settings set another duration.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        admins: Arc<dyn AdminRepository>,
        store: Arc<dyn PasswordResetStore>,
        tokens: Arc<dyn ResetTokenGenerator>,
        hasher: Arc<dyn PasswordHasher>,
        clock: Arc<dyn Clock>,
        mail: MailSlot,
        policy: PasswordPolicy,
        ttl: Duration,
    ) -> Self {
        Self {
            admins,
            store,
            tokens,
            hasher,
            clock,
            mail,
            policy,
            ttl,
        }
    }

    /// The policy a new password must meet, for the form's hints.
    pub fn policy(&self) -> PasswordPolicy {
        self.policy
    }

    /// How long a link works.
    pub fn ttl(&self) -> Duration {
        self.mail
            .reset_link_minutes()
            .map_or(self.ttl, |m| Duration::from_secs(m * 60))
    }

    /// Ask for a reset link for `email`. `link_base` is the public address of
    /// the web app (`https://orquestador.example.com`).
    ///
    /// `Ok` whether or not the account exists; the returned id (for the audit
    /// trail only, never for the response) says which account it was.
    pub async fn request(
        &self,
        email: &str,
        link_base: &str,
    ) -> Result<Option<AdminId>, ResetError> {
        if !self.mail.configured() {
            return Err(ResetError::MailUnavailable);
        }
        let Ok(email) = Email::parse(email) else {
            return Ok(None);
        };
        let Some(account) = self.admins.find_by_email(&email).await.map_err(internal)? else {
            return Ok(None);
        };

        let lang = language(self.admins.as_ref(), &self.mail, &account.id).await;
        let mail = if account.password_hash.is_directory_managed() {
            texts::within(lang, async { directory_mail(&account.email) }).await
        } else {
            let token = self.tokens.generate();
            let ttl = self.ttl();
            let expires_at = self.clock.now() + ttl;
            self.store
                .issue(&account.id, &self.tokens.hash(&token), expires_at)
                .await
                .map_err(internal)?;
            let link = format!(
                "{}{RESET_FORM_PATH}#{token}",
                link_base.trim().trim_end_matches('/')
            );
            texts::within(lang, async {
                reset_mail(&account.email, account.display_name.as_deref(), &link, ttl)
            })
            .await
        };

        // In the background: an SMTP round trip would otherwise make "this
        // account exists" measurably slower than "it does not".
        self.mail.send_detached(mail, "password reset");
        Ok(Some(account.id))
    }

    /// Email `account` (created by an admin, still without a password) the
    /// link to choose its password, replacing any earlier one. Sent in the
    /// foreground: the admin who created the account wants to know if it left.
    pub async fn send_activation(
        &self,
        account: &AdminAccount,
        link_base: &str,
    ) -> Result<(), ResetError> {
        if !self.mail.configured() {
            return Err(ResetError::MailUnavailable);
        }
        let token = self.tokens.generate();
        self.store
            .issue(
                &account.id,
                &self.tokens.hash(&token),
                self.clock.now() + ACTIVATION_TTL,
            )
            .await
            .map_err(internal)?;
        let link = format!(
            "{}{ACTIVATION_FORM_PATH}#{token}",
            link_base.trim().trim_end_matches('/')
        );
        // The account is new and chose no language: the tenant's notice language.
        let mail = texts::within(self.mail.notice_language(), async {
            activation_mail(
                &account.email,
                account.display_name.as_deref(),
                &link,
                ACTIVATION_TTL,
            )
        })
        .await;
        self.mail.send(&mail).await.map_err(ResetError::Internal)
    }

    /// The administrator's entry from the server (the installer, or
    /// `orquestador acceso-admin`), with no password anywhere: if there is no
    /// account, it creates `email`'s as a pending administrator, and gives a
    /// link to choose the password. For an account that already chose it, only
    /// with `force` (it was lost and there is no outgoing mail). With outgoing
    /// mail, the link also reaches the account.
    pub async fn admin_access(
        &self,
        email: &str,
        link_base: &str,
        force: bool,
    ) -> Result<AdminAccess, AccessError> {
        let email =
            Email::parse(email.trim()).map_err(|e| AccessError::InvalidEmail(e.to_string()))?;
        let mut created = false;
        let account = match self
            .admins
            .find_by_email(&email)
            .await
            .map_err(access_internal)?
        {
            Some(account) => account,
            None if self.admins.count().await.map_err(access_internal)? == 0 => {
                let id = self
                    .admins
                    .insert(&NewAdmin {
                        email,
                        password_hash: PasswordHash::pending_activation(),
                        role: Role::admin(),
                    })
                    .await
                    .map_err(access_internal)?;
                created = true;
                self.admins
                    .find_by_id(&id)
                    .await
                    .map_err(access_internal)?
                    .ok_or_else(|| AccessError::Internal("the new admin vanished".into()))?
            }
            None => return Err(AccessError::UnknownAccount),
        };
        if account.password_hash.is_directory_managed() {
            return Err(AccessError::DirectoryAccount);
        }
        let pending = account.password_hash.is_pending_activation();
        if !pending && !force {
            return Ok(AdminAccess {
                admin: account.id,
                created,
                link: None,
                emailed: false,
            });
        }

        let token = self.tokens.generate();
        self.store
            .issue(
                &account.id,
                &self.tokens.hash(&token),
                self.clock.now() + ACTIVATION_TTL,
            )
            .await
            .map_err(access_internal)?;
        let path = if pending {
            ACTIVATION_FORM_PATH
        } else {
            RESET_FORM_PATH
        };
        let link = format!("{}{path}#{token}", link_base.trim().trim_end_matches('/'));
        let emailed = if self.mail.configured() {
            let mail = texts::within(self.mail.notice_language(), async {
                let name = account.display_name.as_deref();
                if pending {
                    activation_mail(&account.email, name, &link, ACTIVATION_TTL)
                } else {
                    reset_mail(&account.email, name, &link, ACTIVATION_TTL)
                }
            })
            .await;
            match self.mail.send(&mail).await {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!("admin access: could not send the email: {e}");
                    false
                }
            }
        } else {
            false
        };
        Ok(AdminAccess {
            admin: account.id,
            created,
            link: Some(link),
            emailed,
        })
    }

    /// The outgoing mail it sends with, for other services' notices.
    pub fn mail_slot(&self) -> MailSlot {
        self.mail.clone()
    }

    /// Whether outgoing mail is configured (an activation needs it).
    pub fn mail_configured(&self) -> bool {
        self.mail.configured()
    }

    /// Whether `token` is a link that still works (for the form to say so
    /// before the person types a new password).
    pub async fn check(&self, token: &str) -> Result<bool, ResetError> {
        let token = token.trim();
        if token.is_empty() {
            return Ok(false);
        }
        Ok(self
            .store
            .find(&self.tokens.hash(token), self.clock.now())
            .await
            .map_err(internal)?
            .is_some())
    }

    /// Set a new password with the link's `token`. Every session of the
    /// account ends, so whoever had the old password is signed out too.
    pub async fn reset(&self, token: &str, new_password: &str) -> Result<AdminId, ResetError> {
        self.policy
            .validate(new_password)
            .map_err(ResetError::WeakPassword)?;
        let token = token.trim();
        if token.is_empty() {
            return Err(ResetError::InvalidToken);
        }
        let hash = self.hasher.hash(new_password).await.map_err(internal)?;
        let token_hash = self.tokens.hash(token);
        // Before using the link: if the account was pending activation, the
        // notice is a welcome and not a password change.
        let before = match self.store.find(&token_hash, self.clock.now()).await {
            Ok(Some(id)) => self.admins.find_by_id(&id).await.ok().flatten(),
            _ => None,
        };
        let id = self
            .store
            .redeem(&token_hash, &hash, self.clock.now())
            .await
            .map_err(internal)?
            .ok_or(ResetError::InvalidToken)?;
        if let Some(account) = before.filter(|a| a.id == id) {
            let lang = language(self.admins.as_ref(), &self.mail, &id).await;
            let mail = texts::within(lang, async {
                let name = account.display_name.as_deref();
                if account.password_hash.is_pending_activation() {
                    welcome_mail(&account.email, name)
                } else {
                    password_changed_mail(&account.email, name)
                }
            })
            .await;
            self.mail.send_detached(mail, "password changed");
        }
        Ok(id)
    }

    /// Notify `account` that an administrator deleted it. Whoever deletes it
    /// calls this, with `lang` read before deleting it.
    pub(crate) async fn notify_deleted(&self, account: &AdminAccount, lang: texts::Language) {
        if account.password_hash.is_pending_activation() {
            // It was never used: the notice would only confuse.
            return;
        }
        let mail = texts::within(lang, async {
            deleted_mail(&account.email, account.display_name.as_deref())
        })
        .await;
        self.mail.send_detached(mail, "account deleted");
    }

    /// The language of the emails to `id` (to read it before deleting the account).
    pub(crate) async fn language_of(&self, id: &AdminId) -> texts::Language {
        language(self.admins.as_ref(), &self.mail, id).await
    }
}

#[cfg(test)]
mod tests;
