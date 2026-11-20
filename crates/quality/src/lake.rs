//! A job's access to its tenant's lake (decisions 0017 and 0030): the
//! tenant's warehouse and credentials of its own, issued per job and
//! narrowed by a session policy to that warehouse. The policies are
//! written in [`crate::lake_policy`]; issuing them is the [`LakeIssuer`]'s
//! (RustFS's STS, in `infrastructure`).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The tenant's warehouse: `qd-tenant-<id>`, the id without hyphens (an
/// id is never renamed, a subdomain can be).
pub fn warehouse(tenant: Uuid) -> String {
    format!("qd-tenant-{}", tenant.simple())
}

pub use crate::lake_policy::*;

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

/// Issues a job's credentials within `policy`, for `warehouse`; `session`
/// names them in the lake's logs.
#[async_trait]
pub trait LakeIssuer: Send + Sync {
    async fn issue(
        &self,
        warehouse: &str,
        session: &str,
        policy: &str,
    ) -> Result<LakeAccess, String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tenants_warehouse_has_its_id_without_hyphens() {
        let tenant = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        assert_eq!(
            warehouse(tenant),
            "qd-tenant-00000000000000000000000000000001"
        );
    }
}
