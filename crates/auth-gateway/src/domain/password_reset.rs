//! Password recovery: the ports behind "forgot your password?".
//!
//! A reset link carries a random token; only its hash is stored, next to the
//! account it unlocks and the instant it stops working. Redeeming the token
//! is single-use and atomic with the password change, so a link replayed after
//! use (or after a newer link was issued) finds nothing.

use std::time::SystemTime;

use crate::domain::{AdminId, PasswordHash, RepositoryError};

/// Port: persistence for pending password-reset links.
#[async_trait::async_trait]
pub trait PasswordResetStore: Send + Sync {
    /// Record a new link for `admin`, replacing any earlier one: only the most
    /// recent email works. Expired links of every account may be pruned too.
    async fn issue(
        &self,
        admin: &AdminId,
        token_hash: &str,
        expires_at: SystemTime,
    ) -> Result<(), RepositoryError>;

    /// The account a still-valid link belongs to, without consuming it.
    async fn find(
        &self,
        token_hash: &str,
        now: SystemTime,
    ) -> Result<Option<AdminId>, RepositoryError>;

    /// Consume a still-valid link and, in the same transaction, set the
    /// account's password to `new_hash`, clear its lockout and end every
    /// session it has. `None` when the link is unknown, used or expired.
    async fn redeem(
        &self,
        token_hash: &str,
        new_hash: &PasswordHash,
        now: SystemTime,
    ) -> Result<Option<AdminId>, RepositoryError>;
}

/// Port: the single-use passes from the common entrance (`entrar.<domain>`) to
/// a company, stored by their hash in the current company.
#[async_trait::async_trait]
pub trait LoginHandoffStore: Send + Sync {
    async fn issue(
        &self,
        admin: &AdminId,
        token_hash: &str,
        expires_at: SystemTime,
    ) -> Result<(), RepositoryError>;

    /// Using it deletes it: it works once. `None` if it does not exist or expired.
    async fn redeem(
        &self,
        token_hash: &str,
        now: SystemTime,
    ) -> Result<Option<AdminId>, RepositoryError>;
}

/// Port: the secret in a reset link and the hash that is stored instead.
pub trait ResetTokenGenerator: Send + Sync {
    /// A fresh, unguessable token for the link.
    fn generate(&self) -> String;

    /// The stored form of `token` (a one-way hash).
    fn hash(&self, token: &str) -> String;
}
