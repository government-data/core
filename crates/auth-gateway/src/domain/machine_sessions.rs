//! A machine's open sessions on the queue's PostgreSQL (decision 0066,
//! point 4): PostgreSQL checks a certificate only at the handshake, so a
//! retired machine's sessions would go on; retiring it ends them.

use crate::domain::RepositoryError;

/// Port: the database sessions opened with a machine's certificate.
#[async_trait::async_trait]
pub trait MachineSessions: Send + Sync {
    /// End every session whose client certificate is the machine `name`'s
    /// (its `CN`); how many were ended.
    async fn end(&self, name: &str) -> Result<u64, RepositoryError>;
}
