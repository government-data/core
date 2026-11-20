//! What verifying the audit chain reads and reports (decision 0070, points
//! 11 and 12): the ports the verifier reads the chain through and keeps
//! each run in, and its report. A break is a sequence number and a kind,
//! never a value of the row.

use async_trait::async_trait;

use crate::audit_chain::ChainRow;
use crate::RepositoryError;

/// A sealed row as the verifier reads it: its number, the hash stored with
/// it, its encoding's version and its fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedRow {
    pub seq: i64,
    pub hash: Vec<u8>,
    pub version: i16,
    pub row: ChainRow,
}

/// A tenant's head row, read in one snapshot with the highest sequence
/// number among its rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainHead {
    pub seq: i64,
    pub hash: Vec<u8>,
    /// Where adoption ended (point 13): rows up to it are chained, not proved.
    pub adopted_seq: i64,
}

/// What [`AuditChainRows::head`] reads in one snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChainTail {
    pub head: Option<ChainHead>,
    /// The highest `chain_seq` among the tenant's rows.
    pub last_row: Option<i64>,
}

/// The rows of a tenant's chain, in sequence order (point 11).
#[async_trait]
pub trait AuditChainRows: Send + Sync {
    /// Every tenant, whose chain is checked.
    async fn tenants(&self) -> Result<Vec<String>, RepositoryError>;
    /// The tenant's head and its last row's number, in one snapshot.
    async fn tail(&self, tenant: &str) -> Result<ChainTail, RepositoryError>;
    /// At most `limit` rows numbered after `after` and up to `through`, in
    /// sequence order.
    async fn rows(
        &self,
        tenant: &str,
        after: i64,
        through: i64,
        limit: u32,
    ) -> Result<Vec<SealedRow>, RepositoryError>;
}

/// Where each verification is kept (`audit_chain_checks`, point 12).
#[async_trait]
pub trait AuditChainChecks: Send + Sync {
    async fn record(&self, report: &ChainReport) -> Result<(), RepositoryError>;
}

/// The kind of a break (point 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakKind {
    /// A number missing.
    Gap,
    /// A row whose hash does not match its fields and the previous hash.
    Hash,
    /// An anchor whose hash differs from the row at its number (step 4).
    AnchorMismatch,
    /// An anchor beyond the database's head (step 4).
    BehindAnchor,
    /// The head row disagreeing with the last row.
    Head,
}

impl BreakKind {
    /// Its name in `audit_chain_checks`, the logs and the report.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gap => "gap",
            Self::Hash => "hash",
            Self::AnchorMismatch => "anchor_mismatch",
            Self::BehindAnchor => "behind_anchor",
            Self::Head => "head",
        }
    }
}

/// The first break of a chain: where, and of what kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainBreak {
    pub seq: i64,
    pub kind: BreakKind,
}

/// One tenant's verification (point 11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainReport {
    pub tenant: String,
    pub rows_checked: i64,
    /// The first and last sequence numbers checked.
    pub first_seq: Option<i64>,
    pub last_seq: Option<i64>,
    /// The head's number and hash, as read.
    pub head: Option<(i64, Vec<u8>)>,
    pub adopted_seq: Option<i64>,
    pub first_break: Option<ChainBreak>,
}
