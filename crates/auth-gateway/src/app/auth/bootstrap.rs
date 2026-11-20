//! Seeding the first administrator from configuration.

use super::*;

/// Application service that seeds the first administrator.
///
/// The password arrives from the environment (a secret), never from the
/// repository, satisfying "bootstrap the first admin with no password in the
/// repo". The operation is idempotent-friendly: it refuses to run once any admin
/// exists, so re-invoking the bootstrap command cannot silently reset an
/// account.
#[derive(Clone)]
pub struct BootstrapService {
    repo: Arc<dyn AdminRepository>,
    hasher: Arc<dyn PasswordHasher>,
    policy: PasswordPolicy,
}

impl std::fmt::Debug for BootstrapService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BootstrapService")
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

/// The result of a bootstrap attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum BootstrapOutcome {
    /// A new admin was created, with the assigned id.
    Created(AdminId),
    /// An administrator already existed; nothing was changed.
    AlreadyInitialized,
}

/// Why a bootstrap attempt failed.
#[derive(Debug)]
pub enum BootstrapError {
    /// The supplied email was invalid.
    InvalidEmail(domain::EmailError),
    /// The supplied password did not meet the policy.
    WeakPassword(PasswordPolicyError),
    /// A hashing or storage failure.
    Internal(String),
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEmail(e) => write!(f, "invalid admin email: {e}"),
            Self::WeakPassword(e) => write!(f, "{e}"),
            Self::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for BootstrapError {}

impl BootstrapService {
    /// Assemble the service from its ports and the password policy.
    pub fn new(
        repo: Arc<dyn AdminRepository>,
        hasher: Arc<dyn PasswordHasher>,
        policy: PasswordPolicy,
    ) -> Self {
        Self {
            repo,
            hasher,
            policy,
        }
    }

    /// Create the first administrator if none exists yet.
    ///
    /// Validates the email and enforces the password policy *before* touching
    /// storage, then no-ops (returning [`BootstrapOutcome::AlreadyInitialized`])
    /// if any admin is already present.
    pub async fn create_first_admin(
        &self,
        email: &str,
        password: &str,
    ) -> Result<BootstrapOutcome, BootstrapError> {
        let email = Email::parse(email).map_err(BootstrapError::InvalidEmail)?;
        self.policy
            .validate(password)
            .map_err(BootstrapError::WeakPassword)?;

        let existing = self
            .repo
            .count()
            .await
            .map_err(|e| BootstrapError::Internal(e.to_string()))?;
        if existing > 0 {
            return Ok(BootstrapOutcome::AlreadyInitialized);
        }

        let password_hash = self
            .hasher
            .hash(password)
            .await
            .map_err(|e| BootstrapError::Internal(e.to_string()))?;
        let id = self
            .repo
            .insert(&NewAdmin {
                email,
                password_hash,
                // The very first administrator is always `admin` — there is
                // no lower-privileged role to choose from before any account
                // exists to grant one.
                role: Role::admin(),
            })
            .await
            .map_err(|e| BootstrapError::Internal(e.to_string()))?;
        Ok(BootstrapOutcome::Created(id))
    }
}
