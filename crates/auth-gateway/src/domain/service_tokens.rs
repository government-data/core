//! Service accounts' tokens (ADR 0016): integrations and scripts call the API
//! with `Authorization: Bearer <access key>.<secret>` instead of a session
//! cookie.

use crate::domain::{AdminId, RepositoryError, Role};

/// Who a valid token speaks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicePrincipal {
    pub service_account_id: String,
    /// Its owner: the token can do at most what the owner can.
    pub owner_id: AdminId,
    pub owner_role: Role,
}

/// Port: check a service account's token.
#[async_trait::async_trait]
pub trait ServiceTokens: Send + Sync {
    /// `Ok(None)` if it is not a token of a live service account (unknown,
    /// wrong secret, expired, or its owner is gone or locked).
    async fn verify(&self, token: &str) -> Result<Option<ServicePrincipal>, RepositoryError>;
}
