//! Administrator authentication use cases.
//!
//! [`LoginService`] verifies an email/password pair with three defenses wired
//! together: argon2 verification (through the [`PasswordHasher`] port), a
//! constant-work path so a nonexistent account cannot be distinguished from a
//! wrong password by timing or response, and progressive lockout at both the
//! account and the client-IP level. [`BootstrapService`] seeds the first
//! administrator from configuration so no password is ever committed to the
//! repository.
//!
//! Both services receive domain ports (as `Arc<dyn _>`) and hold no knowledge of
//! HTTP or storage — the composition root decides which adapters back them.

use std::sync::Arc;

use crate::{
    AdminAccount, AdminRepository, DirectoryAccounts, DirectoryAuthenticator, IpLockoutStore,
    LockoutPolicy, NewAdmin, PasswordHasher, PasswordPolicy, PasswordPolicyError,
};
use domain::{AdminId, Clock, Email, Role};

use crate::app::account_mail::{language, locked_mail};
use crate::app::mail::MailSlot;

mod bootstrap;
mod directory;

pub use bootstrap::*;
pub use directory::*;

/// A single login attempt: the submitted credentials plus the client's address.
///
/// The `client_ip` is the identity used for IP-level lockout. Behind a reverse
/// proxy the delivery layer resolves it from the forwarded headers before
/// building this request.
#[derive(Debug, Clone)]
pub struct LoginRequest {
    /// The submitted email (unparsed — the service normalizes it).
    pub email: String,
    /// The submitted plaintext password.
    pub password: String,
    /// The client's IP address, for per-IP throttling.
    pub client_ip: String,
}

/// The outcome of a rejected login, kept deliberately coarse.
///
/// [`Self::InvalidCredentials`] is returned identically for a nonexistent
/// account and a wrong password — the two must be indistinguishable. Lockout is
/// a *separate* axis ([`Self::TooManyAttempts`]); reporting it does not leak
/// account existence because an IP is throttled even for emails that were never
/// registered.
#[derive(Debug)]
pub enum LoginError {
    /// Wrong password, or no such account — indistinguishable by design.
    InvalidCredentials,
    /// The account or the client IP is currently locked out.
    TooManyAttempts {
        /// Seconds the caller should wait before retrying, if known.
        retry_after_secs: Option<u64>,
    },
    /// The directory (LDAP / AD) could not be reached or queried, so the
    /// credentials could not be checked at all. Not the caller's fault and not
    /// a failed attempt; the delivery layer maps it to a 503. The detail is for
    /// the log only.
    DirectoryUnavailable(String),
    /// An internal failure (hashing or storage). Never leaks specifics to the
    /// caller; the delivery layer maps it to a 500.
    Internal(String),
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCredentials => f.write_str("invalid credentials"),
            Self::TooManyAttempts { .. } => f.write_str("too many attempts"),
            Self::DirectoryUnavailable(msg) => write!(f, "directory unavailable: {msg}"),
            Self::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for LoginError {}

/// The identity of a successfully authenticated administrator: enough for the
/// delivery layer to issue a session ([`crate::app::SessionService::start`]) and
/// record the login to the audit trail, without re-fetching the account.
#[derive(Debug, Clone)]
pub struct AuthenticatedAdmin {
    /// The authenticated administrator's id.
    pub id: AdminId,
    /// The administrator's role, for the session the delivery layer issues
    /// next and for the RBAC middleware that later reads it back.
    pub role: Role,
}

/// Application service for the admin login use case.
///
/// Cloneable (everything inside is an `Arc` or `Copy`), so the delivery layer
/// can hold it as router state.
#[derive(Clone)]
pub struct LoginService {
    repo: Arc<dyn AdminRepository>,
    ip_lockouts: Arc<dyn IpLockoutStore>,
    hasher: Arc<dyn PasswordHasher>,
    clock: Arc<dyn Clock>,
    policy: LockoutPolicy,
    directory: DirectorySlot,
    /// To notify an account that it was locked.
    mail: MailSlot,
}

impl std::fmt::Debug for LoginService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginService")
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl LoginService {
    /// Assemble the service from its ports and the lockout policy.
    pub fn new(
        repo: Arc<dyn AdminRepository>,
        ip_lockouts: Arc<dyn IpLockoutStore>,
        hasher: Arc<dyn PasswordHasher>,
        clock: Arc<dyn Clock>,
        policy: LockoutPolicy,
    ) -> Self {
        Self {
            repo,
            ip_lockouts,
            hasher,
            clock,
            policy,
            directory: DirectorySlot::default(),
            mail: MailSlot::default(),
        }
    }

    /// Email the account when it is locked by failed attempts.
    #[must_use]
    pub fn with_mail(mut self, mail: MailSlot) -> Self {
        self.mail = mail;
        self
    }

    /// Also accept directory (LDAP) accounts. Local accounts keep verifying
    /// with argon2; any email without a local password goes to the directory.
    #[must_use]
    pub fn with_directory(self, directory: DirectoryLogin) -> Self {
        self.directory.set(Some(directory));
        self
    }

    /// The handle that swaps the directory at run time.
    pub fn directory_slot(&self) -> DirectorySlot {
        self.directory.clone()
    }

    /// Attempt a login, returning the authenticated [`AdminId`] on success.
    ///
    /// The flow is ordered to preserve the two security properties:
    ///
    /// 1. **IP lockout first.** A throttled IP is rejected before any account
    ///    lookup, so a flood cannot even probe for accounts.
    /// 2. **Constant work.** Whether or not the email matches an account, the
    ///    service runs exactly one argon2 verification — against the real hash
    ///    or the hasher's reference hash — so response and timing are identical
    ///    for "no such user" and "wrong password".
    /// 3. **Record failures on both axes.** A failure bumps the IP counter
    ///    always and the account counter when the account exists; a success
    ///    clears both.
    pub async fn login(&self, request: LoginRequest) -> Result<AuthenticatedAdmin, LoginError> {
        self.attempt(request, true).await
    }

    /// Like [`Self::login`], but a failure is not recorded: the common entrance
    /// (`entrar.<domain>`) tries the password in every company where the email
    /// is, and one company's password need not be another's. If it works in
    /// none, it records the failure in all of them with
    /// [`Self::record_failure`]. A success clears the counters as always.
    pub async fn check(&self, request: LoginRequest) -> Result<AuthenticatedAdmin, LoginError> {
        self.attempt(request, false).await
    }

    /// Record a failed attempt, as [`Self::login`] would: on the IP and, if it
    /// exists, on the account.
    pub async fn record_failure(&self, request: &LoginRequest) -> Result<(), LoginError> {
        let now = self.clock.now();
        let ip = request.client_ip.as_str();
        let ip_state = self
            .ip_lockouts
            .get(ip)
            .await
            .map_err(|e| LoginError::Internal(e.to_string()))?;
        if let Ok(email) = Email::parse(&request.email) {
            if let Some(account) = self
                .repo
                .find_by_email(&email)
                .await
                .map_err(|e| LoginError::Internal(e.to_string()))?
            {
                self.register_account_failure(&account, now).await?;
            }
        }
        self.record_ip_failure(ip, &ip_state, now).await
    }

    async fn attempt(
        &self,
        request: LoginRequest,
        record: bool,
    ) -> Result<AuthenticatedAdmin, LoginError> {
        let now = self.clock.now();
        let ip = request.client_ip.as_str();

        // 1. IP-level lockout short-circuit.
        let ip_state = self
            .ip_lockouts
            .get(ip)
            .await
            .map_err(|e| LoginError::Internal(e.to_string()))?;
        if self.policy.is_locked(&ip_state, now) {
            return Err(self.too_many_attempts(&ip_state, now));
        }

        // Parse the email. An unparseable email cannot match any account, but we
        // still perform the reference verification below so timing does not
        // reveal that the input never reached storage.
        let account = match Email::parse(&request.email) {
            Ok(email) => self
                .repo
                .find_by_email(&email)
                .await
                .map_err(|e| LoginError::Internal(e.to_string()))?,
            Err(_) => None,
        };

        // Accounts without a local password belong to the directory. With a
        // directory configured, they (and unknown emails) are verified there;
        // without one, they are treated as nonexistent below.
        let local = account.filter(|a| !a.password_hash.is_directory_managed());
        if let (Some(directory), None) = (self.directory.get(), &local) {
            return self
                .directory_login(&directory, &request, ip, &ip_state, now, record)
                .await;
        }
        // An account still waiting for its activation has no password yet:
        // it signs in like an unknown email (same error, same argon2 work).
        let account = local.filter(|a| !a.password_hash.is_pending_activation());

        // 2. Constant-work verification: real hash if the account exists, else
        // the reference hash. Exactly one argon2 evaluation happens either way.
        let hash = account
            .as_ref()
            .map(|a| a.password_hash.clone())
            .unwrap_or_else(|| self.hasher.reference_hash());
        let verified = self
            .hasher
            .verify(&request.password, &hash)
            .await
            .map_err(|e| LoginError::Internal(e.to_string()))?;

        match account {
            // An account matched.
            Some(account) => {
                // A locked account is rejected even with the right password.
                if self.policy.is_locked(&account.lockout, now) {
                    if record {
                        self.record_ip_failure(ip, &ip_state, now).await?;
                    }
                    return Err(self.too_many_attempts(&account.lockout, now));
                }
                if verified {
                    // Success: clear both counters.
                    self.reset_account(&account.id, now).await?;
                    self.reset_ip(ip, now).await?;
                    Ok(AuthenticatedAdmin {
                        id: account.id,
                        role: account.role,
                    })
                } else {
                    // Wrong password: bump the account and IP counters.
                    if record {
                        self.register_account_failure(&account, now).await?;
                        self.record_ip_failure(ip, &ip_state, now).await?;
                    }
                    Err(LoginError::InvalidCredentials)
                }
            }
            // No account: only the IP counter moves. Same error, same work.
            None => {
                if record {
                    self.record_ip_failure(ip, &ip_state, now).await?;
                }
                Err(LoginError::InvalidCredentials)
            }
        }
    }

    /// Record a failed attempt on the account and, if that locks it, email it
    /// (in the background: the response must not take longer because of it,
    /// nor tell whether the account exists).
    async fn register_account_failure(
        &self,
        account: &AdminAccount,
        now: std::time::SystemTime,
    ) -> Result<(), LoginError> {
        let next = self.policy.register_failure(&account.lockout, now);
        self.repo
            .update_lockout(&account.id, &next)
            .await
            .map_err(|e| LoginError::Internal(e.to_string()))?;
        let locks_now =
            !self.policy.is_locked(&account.lockout, now) && self.policy.is_locked(&next, now);
        if locks_now && self.mail.configured() {
            let lock = self.policy.retry_after(&next, now).unwrap_or_default();
            let (repo, mail, account) = (self.repo.clone(), self.mail.clone(), account.clone());
            tokio::spawn(async move {
                let lang = language(repo.as_ref(), &mail, &account.id).await;
                let notice = texts::within(lang, async {
                    locked_mail(
                        &account.email,
                        account.display_name.as_deref(),
                        next.failed_attempts,
                        lock,
                    )
                })
                .await;
                mail.send_detached(notice, "account locked");
            });
        }
        Ok(())
    }

    /// Build a [`LoginError::TooManyAttempts`] carrying the remaining lock time.
    fn too_many_attempts(
        &self,
        state: &crate::LockoutState,
        now: std::time::SystemTime,
    ) -> LoginError {
        LoginError::TooManyAttempts {
            retry_after_secs: self.policy.retry_after(state, now).map(|d| d.as_secs()),
        }
    }

    /// Persist an IP failure computed from its prior state.
    async fn record_ip_failure(
        &self,
        ip: &str,
        prior: &crate::LockoutState,
        now: std::time::SystemTime,
    ) -> Result<(), LoginError> {
        let next = self.policy.register_failure(prior, now);
        self.ip_lockouts
            .put(ip, &next)
            .await
            .map_err(|e| LoginError::Internal(e.to_string()))
    }

    /// Clear an account's lockout counters after a success.
    async fn reset_account(
        &self,
        id: &AdminId,
        _now: std::time::SystemTime,
    ) -> Result<(), LoginError> {
        self.repo
            .update_lockout(id, &crate::LockoutState::clear())
            .await
            .map_err(|e| LoginError::Internal(e.to_string()))
    }

    /// Clear an IP's lockout counters after a success from it.
    async fn reset_ip(&self, ip: &str, _now: std::time::SystemTime) -> Result<(), LoginError> {
        self.ip_lockouts
            .put(ip, &crate::LockoutState::clear())
            .await
            .map_err(|e| LoginError::Internal(e.to_string()))
    }
}

#[cfg(test)]
mod tests;
