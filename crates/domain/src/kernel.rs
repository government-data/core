//! The shared kernel: the few identity values every bounded context names —
//! who acted ([`AdminId`]), their [`Email`] and [`Role`] — plus the
//! [`RepositoryError`] and [`Clock`] ports the whole backend shares. Signing
//! in itself (passwords, sessions, OAuth, the directory) lives in the
//! `auth-gateway` crate, which builds on these.

use std::time::SystemTime;

/// A normalized administrator email address.
///
/// Construction normalizes (trims surrounding whitespace and lowercases) and
/// performs a deliberately minimal structural check — a single `@` with a
/// non-empty local part and a dotted, non-empty domain. Deep RFC-5322
/// validation is intentionally out of scope: the goal is a stable lookup key,
/// not a deliverability guarantee. Because lookups compare the normalized form,
/// `Admin@Example.com ` and `admin@example.com` are the same account.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Email(String);

impl Email {
    /// Parse and normalize an email address.
    pub fn parse(raw: &str) -> Result<Self, EmailError> {
        let normalized = raw.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return Err(EmailError::Empty);
        }

        let (local, domain) = normalized.split_once('@').ok_or(EmailError::MissingAt)?;
        // A second `@`, an empty local part, or a domain without a dot (or with
        // empty labels) is rejected — enough to catch fat-fingered input without
        // pretending to be a full RFC validator.
        let domain_ok = domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.');
        if local.is_empty() || domain.contains('@') || !domain_ok {
            return Err(EmailError::Malformed);
        }

        Ok(Self(normalized))
    }

    /// The normalized address as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Email {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why an email failed to parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmailError {
    /// The input was empty after trimming.
    Empty,
    /// The input had no `@` separator.
    MissingAt,
    /// The input had an `@` but was otherwise structurally invalid.
    Malformed,
}

impl std::fmt::Display for EmailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("email must not be empty"),
            Self::MissingAt => f.write_str("email must contain '@'"),
            Self::Malformed => f.write_str("email is malformed"),
        }
    }
}

impl std::error::Error for EmailError {}

/// An account's role, for the minimal RBAC scheme (bead authapp-e00d47).
///
/// Held as a validated, normalized string rather than a closed Rust enum on
/// purpose: adding a new role (say `"editor"`) is then a data change — insert
/// a row with that value — not a structural one. The enforcement mechanism
/// (the session carrying a `Role`, and whatever middleware the `api` layer
/// builds on top of it) never needs to change to support it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Role(String);

impl Role {
    /// The role bootstrap always assigns to the first administrator.
    pub fn admin() -> Self {
        Self("admin".to_string())
    }

    /// Parse and normalize a role name: trimmed and lowercased, non-empty.
    pub fn parse(raw: &str) -> Result<Self, RoleError> {
        let normalized = raw.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return Err(RoleError::Empty);
        }
        Ok(Self(normalized))
    }

    /// The role name, for persistence or comparison.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why a role name failed to parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleError {
    /// The input was empty after trimming.
    Empty,
}

impl std::fmt::Display for RoleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("role must not be empty"),
        }
    }
}

impl std::error::Error for RoleError {}

/// An opaque administrator identifier.
///
/// Held as text so the domain need not depend on a UUID library; the
/// infrastructure adapter maps it to/from the database's `uuid` column.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AdminId(String);

impl AdminId {
    /// Wrap an identifier string.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AdminId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A storage failure from a repository port.
#[derive(Debug)]
pub enum RepositoryError {
    /// An insert was rejected because the email already exists.
    EmailTaken,
    /// Any other backend failure, described for logs.
    Backend(String),
}

impl std::fmt::Display for RepositoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmailTaken => f.write_str("an admin with that email already exists"),
            Self::Backend(msg) => write!(f, "repository backend error: {msg}"),
        }
    }
}

impl std::error::Error for RepositoryError {}

/// Port: the current wall-clock time.
///
/// Injected so lockout timing is deterministic under test — a fake clock lets a
/// test advance time past a lock without sleeping.
pub trait Clock: Send + Sync {
    /// The current instant.
    fn now(&self) -> SystemTime;
}

/// A cheap syntactic check of an email address, for addresses that are not
/// accounts (notification contacts, senders): one `@`, a dotted domain, no
/// whitespace or control characters, at most 254 characters.
pub fn is_valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    email.len() <= 254
        && !local.is_empty()
        && !domain.contains('@')
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && domain.split('.').count() >= 2
        && domain.split('.').all(|part| !part.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_normalizes_case_and_whitespace() {
        assert_eq!(Role::parse("Admin").unwrap().as_str(), "admin");
        assert_eq!(Role::parse("  editor  ").unwrap().as_str(), "editor");
        assert_eq!(Role::parse("admin").unwrap(), Role::admin());
    }

    #[test]
    fn role_rejects_empty_input() {
        assert_eq!(Role::parse("").unwrap_err(), RoleError::Empty);
        assert_eq!(Role::parse("   ").unwrap_err(), RoleError::Empty);
    }

    #[test]
    fn distinct_roles_are_not_equal() {
        assert_ne!(Role::parse("admin").unwrap(), Role::parse("user").unwrap());
    }

    #[test]
    fn email_normalizes_and_validates() {
        assert_eq!(
            Email::parse("  Admin@Example.COM ").unwrap().as_str(),
            "admin@example.com"
        );
        assert_eq!(Email::parse("").unwrap_err(), EmailError::Empty);
        assert_eq!(
            Email::parse("no-at-sign").unwrap_err(),
            EmailError::MissingAt
        );
        assert_eq!(Email::parse("a@b").unwrap_err(), EmailError::Malformed);
        assert_eq!(
            Email::parse("@example.com").unwrap_err(),
            EmailError::Malformed
        );
    }
}
