//! Verifying the audit chain (decision 0070, point 11): every hash
//! recomputed in Rust (`domain::audit_chain`) from the rows read in
//! sequence order, the numbers dense, the head on the last row. Reports the
//! first break by its sequence number and kind, never a value.

use domain::audit_chain::{genesis, row_hash, ChainHash, CHAIN_VERSION};
use domain::audit_chain_check::{
    AuditChainChecks, AuditChainRows, BreakKind, ChainBreak, ChainReport,
};
use domain::RepositoryError;

/// Rows read at a time.
pub const PAGE: u32 = 5_000;

/// Where a chain is verified from: a sequence number and the hash at it.
/// The genesis today; an archive's checkpoint later (point 15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainStart {
    pub seq: i64,
    pub hash: ChainHash,
}

impl ChainStart {
    /// Before a tenant's first row.
    pub fn genesis(tenant: &str) -> Self {
        Self {
            seq: 0,
            hash: genesis(tenant),
        }
    }
}

/// Verifies one tenant's chain from `start`.
pub async fn verify(
    rows: &dyn AuditChainRows,
    tenant: &str,
    start: ChainStart,
) -> Result<ChainReport, RepositoryError> {
    let tail = rows.tail(tenant).await?;
    let mut report = ChainReport {
        tenant: tenant.to_string(),
        rows_checked: 0,
        first_seq: None,
        last_seq: None,
        head: tail.head.as_ref().map(|h| (h.seq, h.hash.clone())),
        adopted_seq: tail.head.as_ref().map(|h| h.adopted_seq),
        first_break: None,
    };
    let through = tail
        .last_row
        .unwrap_or(start.seq)
        .max(tail.head.as_ref().map_or(start.seq, |h| h.seq));
    let (mut seq, mut prev) = (start.seq, start.hash);
    'pages: while seq < through {
        let page = rows.rows(tenant, seq, through, PAGE).await?;
        if page.is_empty() {
            break;
        }
        for sealed in page {
            let kind = if sealed.seq != seq + 1 {
                Some((seq + 1, BreakKind::Gap))
            } else {
                let hash = row_hash(&prev, sealed.seq, &sealed.row);
                prev = hash;
                (sealed.version != CHAIN_VERSION || sealed.hash != hash)
                    .then_some((sealed.seq, BreakKind::Hash))
            };
            if let Some((at, kind)) = kind {
                report.first_break = Some(ChainBreak { seq: at, kind });
                break 'pages;
            }
            seq = sealed.seq;
            report.first_seq.get_or_insert(seq);
            report.last_seq = Some(seq);
            report.rows_checked += 1;
        }
    }
    if report.first_break.is_none() {
        report.first_break = head_break(&report, seq, &prev, start.seq);
    }
    Ok(report)
}

/// The head disagreeing with the last row checked (`seq`, `prev`).
fn head_break(report: &ChainReport, seq: i64, prev: &ChainHash, start: i64) -> Option<ChainBreak> {
    let agrees = match &report.head {
        Some((head, hash)) => *head == seq && hash.as_slice() == prev,
        None => seq == start,
    };
    (!agrees).then(|| ChainBreak {
        seq: report.head.as_ref().map_or(seq, |(head, _)| *head),
        kind: BreakKind::Head,
    })
}

/// Verifies every tenant's chain from its genesis, keeps each run, and
/// logs a break at error level (point 12) by tenant, number and kind.
pub async fn verify_all(
    rows: &dyn AuditChainRows,
    checks: &dyn AuditChainChecks,
    only: Option<&str>,
) -> Result<Vec<ChainReport>, RepositoryError> {
    let tenants = match only {
        Some(tenant) => vec![tenant.to_string()],
        None => rows.tenants().await?,
    };
    let mut reports = Vec::with_capacity(tenants.len());
    for tenant in tenants {
        let report = verify(rows, &tenant, ChainStart::genesis(&tenant)).await?;
        checks.record(&report).await?;
        if let Some(broken) = report.first_break {
            tracing::error!(
                tenant = %tenant,
                seq = broken.seq,
                kind = broken.kind.as_str(),
                "audit chain broken"
            );
        } else {
            tracing::info!(tenant = %tenant, rows = report.rows_checked, "audit chain verified");
        }
        reports.push(report);
    }
    Ok(reports)
}

#[cfg(test)]
mod tests;
