//! Passwords: the opaque hash, the strength policy and the hasher port.

/// An opaque password hash — the encoded output of a [`PasswordHasher`].
///
/// The domain treats it as an opaque token: it is produced and verified only
/// through the [`PasswordHasher`] port, never inspected here. The concrete
/// adapter (argon2) stores its parameters and salt inside the encoded string
/// (the PHC format), which is exactly why verification needs no side channel.
#[derive(Clone, PartialEq, Eq)]
pub struct PasswordHash(String);

impl PasswordHash {
    /// Wrap an already-encoded hash string produced by a [`PasswordHasher`] (or
    /// loaded from storage).
    pub fn from_encoded(encoded: impl Into<String>) -> Self {
        Self(encoded.into())
    }

    /// The encoded hash string, for persistence.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The marker stored for an account whose password lives in the corporate
    /// directory. It is not a PHC string, so no password can ever verify
    /// against it locally — the login use case routes such accounts to the
    /// directory instead.
    pub fn directory_managed() -> Self {
        Self(Self::DIRECTORY_MARKER.to_string())
    }

    /// Whether this is the [`Self::directory_managed`] marker.
    pub fn is_directory_managed(&self) -> bool {
        self.0 == Self::DIRECTORY_MARKER
    }

    /// The marker stored for an account an admin created that has not chosen
    /// its password yet (it gets an activation link). Like the directory
    /// marker it is not a PHC string, so no password verifies against it; the
    /// login use case treats the account as if it did not exist.
    pub fn pending_activation() -> Self {
        Self(Self::PENDING_MARKER.to_string())
    }

    /// Whether this is the [`Self::pending_activation`] marker.
    pub fn is_pending_activation(&self) -> bool {
        self.0 == Self::PENDING_MARKER
    }

    const DIRECTORY_MARKER: &'static str = "!directory";
    const PENDING_MARKER: &'static str = "!pending";
}

// A hash is a secret-adjacent value; keep it out of logs by not printing its
// contents. The workspace's `missing_debug_implementations` lint still wants a
// `Debug` impl, so provide a redacting one.
impl std::fmt::Debug for PasswordHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PasswordHash(<redacted>)")
    }
}

/// A configurable password-strength policy.
///
/// Every rule is a field, so the composition root builds the policy from
/// configuration (environment) rather than hard-coding thresholds. The defaults
/// ([`PasswordPolicy::recommended`]) follow common guidance: a 12-character
/// floor with mixed character classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasswordPolicy {
    /// Minimum length, counted in Unicode scalar values.
    pub min_length: usize,
    /// Require at least one uppercase letter.
    pub require_uppercase: bool,
    /// Require at least one lowercase letter.
    pub require_lowercase: bool,
    /// Require at least one ASCII digit.
    pub require_digit: bool,
    /// Require at least one non-alphanumeric character.
    pub require_symbol: bool,
}

impl PasswordPolicy {
    /// A sensible default: 12+ chars with upper, lower, and digit required.
    pub const fn recommended() -> Self {
        Self {
            min_length: 12,
            require_uppercase: true,
            require_lowercase: true,
            require_digit: true,
            require_symbol: false,
        }
    }

    /// Check a candidate password against every enabled rule.
    ///
    /// Returns every unmet requirement (not just the first), so a bootstrap tool
    /// can tell the operator all the ways their password fell short in one go.
    pub fn validate(&self, password: &str) -> Result<(), PasswordPolicyError> {
        let mut unmet = Vec::new();

        if password.chars().count() < self.min_length {
            unmet.push(PasswordRequirement::MinLength(self.min_length));
        }
        if self.require_uppercase && !password.chars().any(|c| c.is_ascii_uppercase()) {
            unmet.push(PasswordRequirement::Uppercase);
        }
        if self.require_lowercase && !password.chars().any(|c| c.is_ascii_lowercase()) {
            unmet.push(PasswordRequirement::Lowercase);
        }
        if self.require_digit && !password.chars().any(|c| c.is_ascii_digit()) {
            unmet.push(PasswordRequirement::Digit);
        }
        if self.require_symbol && !password.chars().any(|c| !c.is_alphanumeric()) {
            unmet.push(PasswordRequirement::Symbol);
        }

        if unmet.is_empty() {
            Ok(())
        } else {
            Err(PasswordPolicyError { unmet })
        }
    }
}

/// A single password rule that a candidate failed to satisfy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordRequirement {
    /// The password was shorter than the configured minimum.
    MinLength(usize),
    /// An uppercase letter was required but missing.
    Uppercase,
    /// A lowercase letter was required but missing.
    Lowercase,
    /// A digit was required but missing.
    Digit,
    /// A symbol (non-alphanumeric) was required but missing.
    Symbol,
}

impl std::fmt::Display for PasswordRequirement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MinLength(n) => write!(f, "at least {n} characters"),
            Self::Uppercase => f.write_str("an uppercase letter"),
            Self::Lowercase => f.write_str("a lowercase letter"),
            Self::Digit => f.write_str("a digit"),
            Self::Symbol => f.write_str("a symbol"),
        }
    }
}

/// One or more unmet password requirements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordPolicyError {
    /// Every requirement the candidate password failed to meet.
    pub unmet: Vec<PasswordRequirement>,
}

impl std::fmt::Display for PasswordPolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("password does not meet policy: needs ")?;
        for (i, req) in self.unmet.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{req}")?;
        }
        Ok(())
    }
}

impl std::error::Error for PasswordPolicyError {}

/// Port: hashes and verifies passwords.
///
/// Implemented by an argon2 adapter in `infrastructure`. Hashing and
/// verification are `async` so the adapter can offload the CPU-bound work to a
/// blocking thread pool without blocking the async runtime.
#[async_trait::async_trait]
pub trait PasswordHasher: Send + Sync {
    /// Hash a plaintext password, returning the encoded [`PasswordHash`].
    async fn hash(&self, plaintext: &str) -> Result<PasswordHash, PasswordHashError>;

    /// Verify a plaintext password against an encoded hash.
    ///
    /// Returns `Ok(true)` on a match, `Ok(false)` on a mismatch, and `Err` only
    /// when the hash could not be evaluated (e.g. a corrupt encoding).
    async fn verify(&self, plaintext: &str, hash: &PasswordHash)
        -> Result<bool, PasswordHashError>;

    /// A valid reference hash with the adapter's own parameters.
    ///
    /// The login use case verifies against this when no account matches, so a
    /// request for a nonexistent user performs the same argon2 work — and takes
    /// the same time — as one for a real user with a wrong password. This is the
    /// timing-equalization that stops attackers from enumerating accounts.
    fn reference_hash(&self) -> PasswordHash;
}

/// A password hashing/verification failure (not a mismatch — see
/// [`PasswordHasher::verify`]).
#[derive(Debug)]
pub struct PasswordHashError(pub String);

impl std::fmt::Display for PasswordHashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "password hashing failed: {}", self.0)
    }
}

impl std::error::Error for PasswordHashError {}
