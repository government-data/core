//! The IAM records (ADR 0016): groups, who belongs to them, what is attached
//! to whom, and service accounts.

use chrono::{DateTime, Utc};
use serde::Serialize;

/// Who a policy is attached to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    User,
    Group,
    ServiceAccount,
}

impl PrincipalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Group => "group",
            Self::ServiceAccount => "service_account",
        }
    }
}

/// Someone in a group, and how they got there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupMember {
    pub admin_id: String,
    pub email: String,
    pub display_name: Option<String>,
    /// `manual` (added in Settings) or `directory` (by a directory group).
    pub source: String,
}

/// A group: its people get its policies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub description: String,
    /// The directory groups (DNs, lower case) whose members belong.
    pub directory_groups: Vec<String>,
    pub members: Vec<GroupMember>,
    pub policies: Vec<String>,
}

/// A group's own fields, already validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupFields {
    pub name: String,
    pub description: String,
    pub directory_groups: Vec<String>,
}

/// What an account gets, besides its role: its policies and its groups.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UserAccess {
    pub admin_id: String,
    pub policies: Vec<String>,
    pub groups: Vec<String>,
}

/// A service account: a token for integrations and scripts, owned by an
/// account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceAccount {
    pub id: String,
    pub owner_id: String,
    pub owner_email: String,
    pub name: String,
    /// The token's public part.
    pub access_key: String,
    /// Its own policies: none, its owner's; some, only what both allow.
    pub policies: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// A token freshly made: what is stored and what is shown once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintedToken {
    /// The public part, stored.
    pub access_key: String,
    /// The SHA-256 of the secret, stored.
    pub secret_hash: String,
    /// `<access key>.<secret>`: shown once, never stored.
    pub token: String,
}

/// Port: make service accounts' tokens.
pub trait TokenMint: Send + Sync {
    fn mint(&self) -> MintedToken;
}

/// A new service account, with its secret already hashed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewServiceAccount {
    pub owner_id: String,
    pub name: String,
    pub access_key: String,
    pub secret_hash: String,
    pub expires_at: Option<DateTime<Utc>>,
}
