//! The records the platform keeps.
//!
//! They derive `Serialize` because the API returns them as they are: they are
//! plain reads with no invariants to hide, and copying them into DTOs would
//! protect nothing.

use chrono::{DateTime, Utc};
use serde::Serialize;

/// A secret identifier (a UUID as text).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct SecretId(String);

impl SecretId {
    /// Wrap an identifier.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The identifier as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SecretId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A global secret (a database connection, an API key...): registered once
/// in Settings. The value never leaves here: it is stored encrypted and only
/// opened when it is used.
#[derive(Debug, Clone, Serialize)]
pub struct Secret {
    pub id: SecretId,
    pub name: String,
    pub description: String,
    pub updated_at: DateTime<Utc>,
    /// `value` (a single value) or `database` (a connection, see
    /// [`crate::Connection`]).
    pub kind: String,
    /// Of a connection, what is not secret ([`crate::Connection::public`]);
    /// of a value, `{}`.
    pub meta: serde_json::Value,
}

/// A new secret, already validated and with its value encrypted.
#[derive(Debug, Clone)]
pub struct NewSecret {
    pub name: String,
    pub description: String,
    pub sealed: String,
    pub kind: String,
    pub meta: serde_json::Value,
}
