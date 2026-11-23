//! What a job or a call may do in a lake, said in neutral terms (decision
//! 0078, point 3): never in a lake's policy language. The lake's adapter
//! writes a grant in its own dialect; the core sends that policy with STS
//! `AssumeRole`. Pure: no I/O, no vendor name.

use serde::{Deserialize, Serialize};

/// How much of the warehouse a grant reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LakeRead {
    /// Nothing.
    #[default]
    Nothing,
    /// The catalog and the tables' metadata, no data file: no row.
    Metadata,
    /// Every object and every table's metadata of the warehouse.
    Warehouse,
}

/// One table of the warehouse, where the catalog says its files are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LakeTable {
    pub namespace: String,
    pub name: String,
    /// The table's prefix in the warehouse (`tables/<uuid>`), as the
    /// catalog answered it: never as a worker says.
    pub location: String,
}

/// What a grant allows in one warehouse. Every list may be empty; a grant
/// with nothing in it allows nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LakeGrant {
    pub warehouse: String,
    #[serde(default)]
    pub read: LakeRead,
    /// Namespaces where tables may be created and committed.
    #[serde(default)]
    pub create_tables_in: Vec<String>,
    /// Tables committed to, their files written and deleted.
    #[serde(default)]
    pub write_tables: Vec<LakeTable>,
    /// Tables dropped with their files, and their namespace once empty.
    #[serde(default)]
    pub drop_tables: Vec<LakeTable>,
    /// Whether namespaces may be made in the catalog.
    #[serde(default)]
    pub make_namespaces: bool,
    /// A prefix under which objects may be written, once each; not read.
    #[serde(default)]
    pub write_objects_under: Option<String>,
}

/// Why a grant cannot be written in any lake's dialect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LakeGrantError {
    /// The warehouse is not a bucket's name (3 to 63 of `a-z`, `0-9`, `-`).
    Warehouse,
    /// A namespace or table name is not `a-z`, `0-9`, `_`, starting with a
    /// letter.
    Name(String),
    /// A location or prefix is absolute, empty, or climbs out (`..`).
    Path(String),
}

impl std::fmt::Display for LakeGrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Warehouse => f.write_str("the warehouse is not a bucket's name"),
            Self::Name(n) => write!(f, "not a namespace or table name: {n:?}"),
            Self::Path(p) => write!(f, "not a relative path in the warehouse: {p:?}"),
        }
    }
}

impl std::error::Error for LakeGrantError {}

impl LakeGrant {
    /// A grant on `warehouse` that allows nothing yet.
    pub fn on(warehouse: &str) -> Self {
        Self {
            warehouse: warehouse.to_string(),
            ..Self::default()
        }
    }

    /// Reading the whole warehouse.
    pub fn read_only(warehouse: &str) -> Self {
        Self {
            read: LakeRead::Warehouse,
            ..Self::on(warehouse)
        }
    }

    /// The catalog and the tables' metadata, no row.
    pub fn metadata(warehouse: &str) -> Self {
        Self {
            read: LakeRead::Metadata,
            ..Self::on(warehouse)
        }
    }

    /// Whether it allows nothing at all.
    pub fn is_empty(&self) -> bool {
        self.read == LakeRead::Nothing
            && self.create_tables_in.is_empty()
            && self.write_tables.is_empty()
            && self.drop_tables.is_empty()
            && !self.make_namespaces
            && self.write_objects_under.is_none()
    }

    /// Whether every name and path in it can be written in a policy as it
    /// is: nothing a lake's dialect would read as a pattern or a way out.
    pub fn check(&self) -> Result<(), LakeGrantError> {
        if !is_bucket(&self.warehouse) {
            return Err(LakeGrantError::Warehouse);
        }
        for namespace in &self.create_tables_in {
            name(namespace)?;
        }
        for table in self.write_tables.iter().chain(&self.drop_tables) {
            name(&table.namespace)?;
            name(&table.name)?;
            path(&table.location)?;
        }
        if let Some(prefix) = &self.write_objects_under {
            path(prefix)?;
        }
        Ok(())
    }
}

fn is_bucket(s: &str) -> bool {
    (3..=63).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

fn name(s: &str) -> Result<(), LakeGrantError> {
    let mut bytes = s.bytes();
    let first = bytes.next().is_some_and(|b| b.is_ascii_lowercase());
    let rest = bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if first && rest && s.len() <= 128 {
        Ok(())
    } else {
        Err(LakeGrantError::Name(s.to_string()))
    }
}

fn path(s: &str) -> Result<(), LakeGrantError> {
    let parts_ok = s.trim_end_matches('/').split('/').all(|part| {
        !part.is_empty()
            && part != ".."
            && part != "."
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    });
    if parts_ok && !s.starts_with('/') {
        Ok(())
    } else {
        Err(LakeGrantError::Path(s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WH: &str = "qd-tenant-00000000000000000000000000000001";

    fn table() -> LakeTable {
        LakeTable {
            namespace: "clients_bronze".into(),
            name: "clients".into(),
            location: "tables/2537f6fb-9b6a-4f5b-be14-25ce1f655da7".into(),
        }
    }

    #[test]
    fn a_grant_round_trips_through_json() {
        let grant = LakeGrant {
            create_tables_in: vec!["clients_bronze".into()],
            write_tables: vec![table()],
            drop_tables: vec![table()],
            make_namespaces: true,
            write_objects_under: Some("manifests/".into()),
            ..LakeGrant::read_only(WH)
        };
        let json = serde_json::to_string(&grant).unwrap();
        assert!(json.contains(r#""read":"warehouse""#), "{json}");
        let back: LakeGrant = serde_json::from_str(&json).unwrap();
        assert_eq!(back, grant);
        assert_eq!(back.check(), Ok(()));
    }

    #[test]
    fn what_is_left_out_allows_nothing() {
        let grant: LakeGrant = serde_json::from_str(&format!(r#"{{"warehouse":"{WH}"}}"#)).unwrap();
        assert!(grant.is_empty());
        assert_eq!(grant, LakeGrant::on(WH));
        assert!(!LakeGrant::metadata(WH).is_empty());
        assert_eq!(LakeGrant::metadata(WH).read, LakeRead::Metadata);
    }

    #[test]
    fn a_warehouse_must_be_a_buckets_name() {
        for bad in ["", "ab", "Upper", "-lead", "trail-", "a*b", "qd/tenant"] {
            assert_eq!(
                LakeGrant::on(bad).check(),
                Err(LakeGrantError::Warehouse),
                "{bad}"
            );
        }
        assert_eq!(LakeGrant::on(WH).check(), Ok(()));
    }

    #[test]
    fn names_and_paths_are_never_patterns_or_ways_out() {
        let with = |f: &dyn Fn(&mut LakeGrant)| {
            let mut grant = LakeGrant::on(WH);
            f(&mut grant);
            grant.check()
        };
        assert!(matches!(
            with(&|g| g.create_tables_in = vec!["*".into()]),
            Err(LakeGrantError::Name(_))
        ));
        assert!(matches!(
            with(&|g| g.create_tables_in = vec!["1st".into()]),
            Err(LakeGrantError::Name(_))
        ));
        for bad in ["/tables/x", "tables/../other", "tables/*", "", "tables//x"] {
            let mut t = table();
            t.location = bad.into();
            assert!(
                matches!(
                    with(&|g| g.write_tables = vec![t.clone()]),
                    Err(LakeGrantError::Path(_))
                ),
                "{bad}"
            );
        }
        assert!(matches!(
            with(&|g| g.write_objects_under = Some("../x".into())),
            Err(LakeGrantError::Path(_))
        ));
        assert_eq!(
            with(&|g| g.write_objects_under = Some("manifests/".into())),
            Ok(())
        );
    }
}
