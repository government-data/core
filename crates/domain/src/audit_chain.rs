//! The audit trail's hash chain, encoding version 1 (decision 0070, point 2).
//!
//! Pure: it turns a row's fields, the previous row's hash and the row's
//! sequence number into the row's hash. The database's trigger computes the
//! same encoding in SQL (`audit_row_v1`); this one is the verifier's, so a
//! function replaced in the database cannot vouch for itself (point 8). An
//! equivalence test runs both over the same rows.

use sha2::{Digest, Sha256};

/// A SHA-256 digest: a row's hash, or the genesis.
pub type ChainHash = [u8; 32];

/// The encoding's version, stored per row in `chain_version`.
pub const CHAIN_VERSION: i16 = 1;

/// The fields hashed, in the encoding's order (point 2). A column added to
/// the audit later is appended here and to the SQL function in the same
/// migration (point 3).
pub const FIELDS: [&str; 16] = [
    "tenant_id",
    "id",
    "event_type",
    "admin_id",
    "email_attempted",
    "ip",
    "user_agent",
    "occurred_at",
    "action",
    "resource",
    "service_account",
    "job_id",
    "purpose",
    "reference",
    "rows_shown",
    "fields_shown",
];

/// The chain's own columns, which are not hashed.
pub const CHAIN_COLUMNS: [&str; 3] = ["chain_seq", "chain_hash", "chain_version"];

const PREFIX: &[u8] = b"qd-audit-chain/1";
const GENESIS: &str = "qd-audit-chain/genesis/";

/// One audit row as the chain sees it. Uuids are in their lowercase text
/// form; `occurred_at` is microseconds since the Unix epoch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChainRow {
    pub tenant_id: String,
    pub id: String,
    pub event_type: String,
    pub admin_id: Option<String>,
    pub email_attempted: Option<String>,
    pub ip: String,
    pub user_agent: Option<String>,
    pub occurred_at_micros: i64,
    pub action: Option<String>,
    pub resource: Option<String>,
    pub service_account: Option<String>,
    pub job_id: Option<String>,
    pub purpose: Option<String>,
    pub reference: Option<String>,
    pub rows_shown: Option<i64>,
    pub fields_shown: Option<Vec<Option<String>>>,
}

impl ChainRow {
    /// Each field of [`FIELDS`], in order, with its value's bytes, or `None`
    /// for a NULL (absent from the encoding).
    pub fn fields(&self) -> [(&'static str, Option<Vec<u8>>); 16] {
        let text = |v: &Option<String>| v.as_ref().map(|s| s.as_bytes().to_vec());
        [
            ("tenant_id", Some(self.tenant_id.as_bytes().to_vec())),
            ("id", Some(self.id.as_bytes().to_vec())),
            ("event_type", Some(self.event_type.as_bytes().to_vec())),
            ("admin_id", text(&self.admin_id)),
            ("email_attempted", text(&self.email_attempted)),
            ("ip", Some(self.ip.as_bytes().to_vec())),
            ("user_agent", text(&self.user_agent)),
            (
                "occurred_at",
                Some(self.occurred_at_micros.to_be_bytes().to_vec()),
            ),
            ("action", text(&self.action)),
            ("resource", text(&self.resource)),
            ("service_account", text(&self.service_account)),
            ("job_id", text(&self.job_id)),
            ("purpose", text(&self.purpose)),
            ("reference", text(&self.reference)),
            (
                "rows_shown",
                self.rows_shown.map(|n| n.to_be_bytes().to_vec()),
            ),
            ("fields_shown", self.fields_shown.as_deref().map(texts)),
        ]
    }
}

/// An array: its element count, then each element as its length and bytes,
/// a NULL element as length −1.
fn texts(items: &[Option<String>]) -> Vec<u8> {
    let mut out = len(items.len()).to_vec();
    for item in items {
        match item {
            None => out.extend_from_slice(&(-1i32).to_be_bytes()),
            Some(s) => {
                out.extend_from_slice(&len(s.len()));
                out.extend_from_slice(s.as_bytes());
            }
        }
    }
    out
}

/// A length as 4 bytes big-endian (PostgreSQL's `int4send`).
fn len(n: usize) -> [u8; 4] {
    i32::try_from(n).unwrap_or(i32::MAX).to_be_bytes()
}

fn sha256(bytes: &[u8]) -> ChainHash {
    Sha256::digest(bytes).into()
}

/// The hash before a tenant's first row: SHA-256 of
/// `qd-audit-chain/genesis/<tenant id>`.
pub fn genesis(tenant_id: &str) -> ChainHash {
    sha256(format!("{GENESIS}{tenant_id}").as_bytes())
}

/// The bytes hashed for row `seq` after `prev` (point 2).
pub fn row_bytes(prev: &ChainHash, seq: i64, row: &ChainRow) -> Vec<u8> {
    let mut out = PREFIX.to_vec();
    out.extend_from_slice(prev);
    out.extend_from_slice(&seq.to_be_bytes());
    for (name, value) in row.fields() {
        if let Some(value) = value {
            out.extend_from_slice(&len(name.len()));
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&sha256(&value));
        }
    }
    out
}

/// Row `seq`'s hash after `prev`.
pub fn row_hash(prev: &ChainHash, seq: i64, row: &ChainRow) -> ChainHash {
    sha256(&row_bytes(prev, seq, row))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> ChainRow {
        ChainRow {
            tenant_id: "00000000-0000-0000-0000-000000000001".into(),
            id: "00000000-0000-0000-0000-0000000000aa".into(),
            event_type: "login_failed".into(),
            ip: "203.0.113.1".into(),
            occurred_at_micros: 1_800_000_000_000_000,
            ..ChainRow::default()
        }
    }

    #[test]
    fn the_fields_follow_the_list() {
        let names: Vec<_> = row().fields().iter().map(|(n, _)| *n).collect();
        assert_eq!(names, FIELDS);
    }

    #[test]
    fn a_null_field_is_absent() {
        let bytes = row_bytes(&genesis("t"), 1, &row());
        let mut expected = PREFIX.to_vec();
        expected.extend_from_slice(&genesis("t"));
        expected.extend_from_slice(&1i64.to_be_bytes());
        for (name, value) in [
            ("tenant_id", row().tenant_id.into_bytes()),
            ("id", row().id.into_bytes()),
            ("event_type", b"login_failed".to_vec()),
            ("ip", b"203.0.113.1".to_vec()),
            (
                "occurred_at",
                1_800_000_000_000_000i64.to_be_bytes().to_vec(),
            ),
        ] {
            expected.extend_from_slice(&(name.len() as u32).to_be_bytes());
            expected.extend_from_slice(name.as_bytes());
            expected.extend_from_slice(&sha256(&value));
        }
        assert_eq!(bytes, expected);
    }

    #[test]
    fn an_array_counts_its_elements_and_marks_a_null_one() {
        assert_eq!(
            texts(&[Some("ñ".into()), None]),
            [
                &2i32.to_be_bytes()[..],
                &2i32.to_be_bytes(),
                "ñ".as_bytes(),
                &(-1i32).to_be_bytes()
            ]
            .concat()
        );
    }

    #[test]
    fn every_change_changes_the_hash() {
        let base = row_hash(&genesis("t"), 1, &row());
        assert_ne!(base, row_hash(&genesis("u"), 1, &row()));
        assert_ne!(base, row_hash(&genesis("t"), 2, &row()));
        let mut edited = row();
        edited.ip = "203.0.113.2".into();
        assert_ne!(base, row_hash(&genesis("t"), 1, &edited));
        // A field moved to another column is another row.
        let mut moved = row();
        moved.action = Some("login_failed".into());
        moved.event_type = String::new();
        assert_ne!(base, row_hash(&genesis("t"), 1, &moved));
    }
}
