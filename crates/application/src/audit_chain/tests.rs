//! The verifier on synthetic chains held in memory (decision 0070, step 2).

use std::sync::Mutex;

use async_trait::async_trait;
use domain::audit_chain::{genesis, row_hash, ChainRow};
use domain::audit_chain_check::{ChainHead, ChainTail, SealedRow};

use super::*;

const TENANT: &str = "00000000-0000-0000-0000-000000000001";

#[derive(Default)]
struct Chain {
    rows: Vec<SealedRow>,
    head: Option<ChainHead>,
    kept: Mutex<Vec<ChainReport>>,
}

fn row(n: i64) -> ChainRow {
    ChainRow {
        tenant_id: TENANT.into(),
        id: format!("00000000-0000-0000-0000-{n:012}"),
        event_type: "credentials_issued".into(),
        ip: "203.0.113.9".into(),
        occurred_at_micros: 1_800_000_000_000_000 + n,
        action: Some("job:extract".into()),
        ..ChainRow::default()
    }
}

/// `n` rows sealed as the trigger seals them, the head on the last.
fn chain(n: i64) -> Chain {
    let mut c = Chain::default();
    let mut prev = genesis(TENANT);
    for seq in 1..=n {
        prev = row_hash(&prev, seq, &row(seq));
        c.rows.push(SealedRow {
            seq,
            hash: prev.to_vec(),
            version: 1,
            row: row(seq),
        });
    }
    c.head = Some(ChainHead {
        seq: n,
        hash: prev.to_vec(),
        adopted_seq: 0,
    });
    c
}

impl Chain {
    /// Rewrites every hash from row `from` on, and the head, as an owner
    /// who disabled the guard would.
    fn reseal_from(&mut self, from: usize) {
        let mut prev = if from == 0 {
            genesis(TENANT)
        } else {
            self.rows[from - 1].hash.clone().try_into().unwrap()
        };
        for sealed in &mut self.rows[from..] {
            prev = row_hash(&prev, sealed.seq, &sealed.row);
            sealed.hash = prev.to_vec();
        }
        self.head.as_mut().unwrap().hash = prev.to_vec();
    }
}

#[async_trait]
impl AuditChainRows for Chain {
    async fn tenants(&self) -> Result<Vec<String>, RepositoryError> {
        Ok(vec![TENANT.into()])
    }
    async fn tail(&self, _: &str) -> Result<ChainTail, RepositoryError> {
        Ok(ChainTail {
            head: self.head.clone(),
            last_row: self.rows.iter().map(|r| r.seq).max(),
        })
    }
    async fn rows(
        &self,
        _: &str,
        after: i64,
        through: i64,
        limit: u32,
    ) -> Result<Vec<SealedRow>, RepositoryError> {
        let mut rows: Vec<_> = self
            .rows
            .iter()
            .filter(|r| r.seq > after && r.seq <= through)
            .cloned()
            .collect();
        rows.sort_by_key(|r| r.seq);
        rows.truncate(limit as usize);
        Ok(rows)
    }
}

#[async_trait]
impl AuditChainChecks for Chain {
    async fn record(&self, report: &ChainReport) -> Result<(), RepositoryError> {
        self.kept.lock().unwrap().push(report.clone());
        Ok(())
    }
}

async fn first_break(c: &Chain) -> Option<ChainBreak> {
    verify(c, TENANT, ChainStart::genesis(TENANT))
        .await
        .unwrap()
        .first_break
}

fn at(seq: i64, kind: BreakKind) -> Option<ChainBreak> {
    Some(ChainBreak { seq, kind })
}

#[tokio::test]
async fn an_unbroken_chain_verifies_across_pages() {
    let c = chain(i64::from(PAGE) + 3);
    let report = verify(&c, TENANT, ChainStart::genesis(TENANT))
        .await
        .unwrap();
    assert_eq!(report.first_break, None);
    assert_eq!(report.rows_checked, i64::from(PAGE) + 3);
    assert_eq!(
        (report.first_seq, report.last_seq),
        (Some(1), Some(i64::from(PAGE) + 3))
    );
}

#[tokio::test]
async fn an_edited_row_is_a_hash_break_at_its_number() {
    let mut c = chain(10);
    c.rows[6].row.ip = "198.51.100.1".into();
    assert_eq!(first_break(&c).await, at(7, BreakKind::Hash));
}

#[tokio::test]
async fn a_row_of_an_unknown_encoding_is_a_hash_break() {
    let mut c = chain(3);
    c.rows[1].version = 2;
    assert_eq!(first_break(&c).await, at(2, BreakKind::Hash));
}

#[tokio::test]
async fn a_removed_row_is_a_gap() {
    let mut c = chain(10);
    c.rows.remove(4);
    assert_eq!(first_break(&c).await, at(5, BreakKind::Gap));
}

#[tokio::test]
async fn a_chain_rewritten_from_row_5_passes_without_anchors() {
    let mut c = chain(10);
    c.rows[4].row.resource = Some("source/other".into());
    c.reseal_from(4);
    assert_eq!(first_break(&c).await, None, "what the anchors are for");
}

#[tokio::test]
async fn the_head_moved_alone_is_a_head_break() {
    let mut c = chain(10);
    c.head.as_mut().unwrap().seq = 12;
    assert_eq!(first_break(&c).await, at(12, BreakKind::Head));
    let mut c = chain(10);
    c.head.as_mut().unwrap().hash = vec![0; 32];
    assert_eq!(first_break(&c).await, at(10, BreakKind::Head));
}

#[tokio::test]
async fn rows_and_no_head_is_a_head_break_and_neither_verifies() {
    let mut c = chain(2);
    c.head = None;
    assert_eq!(first_break(&c).await, at(2, BreakKind::Head));
    assert_eq!(first_break(&Chain::default()).await, None);
}

#[tokio::test]
async fn every_run_is_kept() {
    let mut c = chain(4);
    c.rows.remove(1);
    let reports = verify_all(&c, &c, None).await.unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(*c.kept.lock().unwrap(), reports);
    assert_eq!(reports[0].first_break, at(2, BreakKind::Gap));
    assert_eq!(reports[0].rows_checked, 1);
}
