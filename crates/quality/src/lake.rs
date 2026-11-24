//! A job's access to the lake (decisions 0017, 0030 and 0078): the
//! warehouse and credentials of its own, issued per job within a grant
//! ([`LakeGrant`]), which the lake's adapter writes in its own policy
//! language; issuing them is the [`LakeIssuer`]'s (the server's, over the
//! lake's STS).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// The lake's warehouse: one installation is one organisation (decision
/// 0076, point 5), so it is a fixed name, the one the main tenant's had
/// (`qd-tenant-<id>`; S3 renames no bucket).
pub const WAREHOUSE: &str = "qd-tenant-00000000000000000000000000000001";

pub use contracts::{LakeGrant, LakeRead, LakeTable};

/// Where a table's files are, as the warehouse's catalog says: never as a
/// worker says (decision 0037); and its namespace, made by the application.
#[async_trait]
pub trait TableCatalog: Send + Sync {
    /// Creates `namespace` unless it is there.
    async fn ensure_namespace(&self, warehouse: &str, namespace: &str) -> Result<(), String>;

    /// The tables of `namespace`, by name; none if it is not there.
    async fn tables(&self, warehouse: &str, namespace: &str) -> Result<Vec<String>, String>;

    /// The table's prefix in the warehouse bucket (`tables/<uuid>`), or
    /// `None` if there is no such table.
    async fn table_prefix(
        &self,
        warehouse: &str,
        namespace: &str,
        table: &str,
    ) -> Result<Option<String>, String>;

    /// Deletes the table with its files, and its namespace if it is left
    /// empty (decision 0055); `false` if there was no such table.
    async fn drop_table(
        &self,
        warehouse: &str,
        namespace: &str,
        table: &str,
    ) -> Result<bool, String>;
}

/// What a worker needs to attach the warehouse: the lake's address (its S3
/// endpoint; the Iceberg catalog is under `/iceberg`) and temporary keys.
/// The secret and the token never show in `Debug`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LakeAccess {
    pub endpoint: String,
    pub warehouse: String,
    pub access_key: String,
    pub secret_key: String,
    pub session_token: String,
    /// RFC 3339.
    pub expires_at: String,
}

impl std::fmt::Debug for LakeAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LakeAccess")
            .field("endpoint", &self.endpoint)
            .field("warehouse", &self.warehouse)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// Issues a job's credentials within `grant`, for its warehouse; `session`
/// names them in the lake's logs.
#[async_trait]
pub trait LakeIssuer: Send + Sync {
    async fn issue(&self, session: &str, grant: &LakeGrant) -> Result<LakeAccess, String>;
}

/// A job that reads the warehouse and writes nothing: an evaluation, a rule
/// test, a console statement.
pub fn read_grant(warehouse: &str) -> LakeGrant {
    LakeGrant::read_only(warehouse)
}

/// A table of `namespace`, where the catalog says its files are.
pub fn table(namespace: &str, name: &str, location: &str) -> LakeTable {
    LakeTable {
        namespace: namespace.into(),
        name: name.into(),
        location: location.into(),
    }
}

/// An extraction's grant (decisions 0037 and 0061): read the warehouse;
/// create and commit tables in `placed`'s namespaces (bronze's and
/// silver's); and, once a table exists, write its files where the catalog
/// says they are. `placed` is each table's namespace, name and location if
/// it is there.
pub fn extract_grant(warehouse: &str, placed: &[(String, String, Option<String>)]) -> LakeGrant {
    LakeGrant {
        create_tables_in: placed.iter().map(|(ns, _, _)| ns.clone()).collect(),
        write_tables: placed
            .iter()
            .filter_map(|(ns, name, at)| at.as_deref().map(|at| table(ns, name, at)))
            .collect(),
        ..LakeGrant::read_only(warehouse)
    }
}

/// A maintain job's grant (decisions 0039 and 0061): read the warehouse;
/// commit to and write or delete the files of `tables` (namespace, name,
/// location) only; create nothing.
pub fn maintain_grant(warehouse: &str, tables: &[(String, String, String)]) -> LakeGrant {
    LakeGrant {
        write_tables: tables
            .iter()
            .map(|(ns, name, at)| table(ns, name, at))
            .collect(),
        ..LakeGrant::read_only(warehouse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_extraction_writes_the_files_of_the_tables_already_there() {
        let placed = [
            (
                "a_bronze".to_string(),
                "t".to_string(),
                Some("tables/u-1".to_string()),
            ),
            ("a_silver".to_string(), "t".to_string(), None),
        ];
        let grant = extract_grant(WAREHOUSE, &placed);
        assert_eq!(grant.read, LakeRead::Warehouse);
        assert_eq!(grant.create_tables_in, ["a_bronze", "a_silver"]);
        assert_eq!(grant.write_tables, [table("a_bronze", "t", "tables/u-1")]);
        assert_eq!(grant.check(), Ok(()));
        let maintain = maintain_grant(
            WAREHOUSE,
            &[("a_bronze".into(), "t".into(), "tables/u-1".into())],
        );
        assert!(maintain.create_tables_in.is_empty() && !maintain.make_namespaces);
        assert_eq!(maintain.write_tables, grant.write_tables);
    }

    #[test]
    fn the_warehouse_keeps_the_main_tenants_name() {
        assert_eq!(WAREHOUSE, "qd-tenant-00000000000000000000000000000001");
    }
}
