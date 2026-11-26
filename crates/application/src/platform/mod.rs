//! The platform's use cases besides identity: the session's actor and the
//! IAM records (ADR 0016), global secrets and the updates.
//!
//! [`PlatformService`] is the only door: each operation of a person receives
//! their [`Actor`] and checks the action it needs before touching anything.

use std::sync::Arc;

use platform::{PlatformRepository, RepositoryError};

mod grantees;
mod module_policies;
mod permissions;
mod secrets;
mod updates;

pub use grantees::PlatformGrantees;
pub use module_policies::{ModuleActionView, OfModule};
pub use permissions::{CreatedServiceAccount, GroupChange, PolicyView};
pub use platform::{MintedToken, TokenMint};

/// Why it could not be done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformError {
    /// It does not exist.
    NotFound,
    /// The actor's policies do not reach this operation.
    Forbidden,
    /// The request is not valid (a field, a name...).
    Invalid(String),
    /// It clashes with the current state (a repeated name...).
    Conflict(String),
    /// An internal failure; the detail goes to the log, not to the client.
    Internal(String),
}

impl std::fmt::Display for PlatformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str(&texts::tr!("app.platform.notFound")),
            Self::Forbidden => f.write_str(&texts::tr!("app.platform.forbidden")),
            Self::Invalid(m) | Self::Conflict(m) => f.write_str(m),
            Self::Internal(m) => write!(f, "internal error: {m}"),
        }
    }
}

impl std::error::Error for PlatformError {}

impl From<RepositoryError> for PlatformError {
    fn from(e: RepositoryError) -> Self {
        match e {
            RepositoryError::NotFound => Self::NotFound,
            RepositoryError::Conflict(m) => Self::Conflict(m),
            RepositoryError::Backend(m) => Self::Internal(m),
        }
    }
}

type R<T> = Result<T, PlatformError>;

fn require(cond: bool, field: &str, msg: &str) -> R<()> {
    if cond {
        Ok(())
    } else {
        Err(PlatformError::Invalid(format!("{field}: {msg}")))
    }
}

/// The platform's application service. Cloneable, for the router's state.
#[derive(Clone)]
pub struct PlatformService {
    repo: Arc<dyn PlatformRepository>,
    /// Encrypts the secrets' values (without it, in clear: only tests and
    /// development).
    cipher: Option<Arc<dyn crate::settings::SecretCipher>>,
    /// Makes the service accounts' tokens.
    tokens: Option<Arc<dyn TokenMint>>,
    /// The updates' maintenance (without it, there is never maintenance).
    updates: Option<crate::UpdatesService>,
    /// Each account's Actor in memory (ADR 0013): without querying the
    /// database on every request.
    actors: Arc<crate::actors::ActorCache>,
}

impl std::fmt::Debug for PlatformService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformService").finish_non_exhaustive()
    }
}

impl PlatformService {
    /// Build the service on its ports.
    pub fn new(repo: Arc<dyn PlatformRepository>) -> Self {
        Self {
            repo,
            cipher: None,
            tokens: None,
            updates: None,
            actors: Arc::new(crate::actors::ActorCache::default()),
        }
    }

    /// Encrypt the secrets with the Settings secrets key.
    pub fn with_cipher(mut self, cipher: Arc<dyn crate::settings::SecretCipher>) -> Self {
        self.cipher = Some(cipher);
        self
    }

    /// With what makes the service accounts' tokens.
    pub fn with_tokens(mut self, tokens: Arc<dyn TokenMint>) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// With the updates' maintenance.
    pub fn with_updates(mut self, updates: crate::UpdatesService) -> Self {
        self.updates = Some(updates);
        self
    }
}
