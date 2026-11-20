//! What the platform keeps besides identity: global secrets and the database
//! connections among them, the IAM records, and the license usage.
//!
//! * the records ([`Secret`], [`NewSecret`]; [`Group`], [`ServiceAccount`]…);
//! * database connections as secrets ([`Connection`]);
//! * the persistence port [`PlatformRepository`].
//!
//! No HTTP or SQL: the Postgres adapter lives in `infrastructure`, the routes
//! in `api`, and the use cases in `application`.

mod connection;
mod iam;
mod records;
mod repository;

pub use crate::iam::{
    Group, GroupFields, GroupMember, MintedToken, NewServiceAccount, PrincipalKind, ServiceAccount,
    TokenMint, UserAccess,
};
pub use connection::{Connection, CONNECTION_KIND};
pub use records::{NewSecret, Secret, SecretId};
pub use repository::{PlatformRepository, RepositoryError};
