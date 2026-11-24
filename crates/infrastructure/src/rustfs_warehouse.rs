//! A tenant's warehouse on the main lake (0028's "a tenant's warehouse";
//! 0066 step 11, the user's ruling of 2026-11-13 on the warehouse): the
//! bucket `qd-tenant-<id>` when `HEAD` says it is missing, then its Iceberg
//! table bucket, every call signed with the main lake's platform admin
//! keys, never root. Verified with RustFS 1.0.5
//! (`tools/verify/rustfs_warehouse.sh`): the table bucket of a missing
//! bucket is refused and makes none, so the bucket goes first; both answer
//! 200 again on a second run and keep the catalog's namespaces and tables;
//! `GET` of a table bucket says nothing of the bucket. Versioning is the
//! replication's step, not this one. No key or signature is logged.

use auth_gateway::{Lake, ReplicationStep};
use domain::RepositoryError;

use crate::rustfs_admin::{code, RustfsLakeAdmin};
use crate::rustfs_replication::{call, Answer};

/// What making one tenant's warehouse on the main lake came to. Holds no
/// key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarehouseOutcome {
    /// It was not there and was made, with its table bucket.
    Made,
    /// It was there; its table bucket was asked for again.
    Present,
    /// The main lake's admin keys are not kept: nothing was asked.
    NoKeys,
    /// A step (the bucket or its table bucket) was refused or not reached.
    Failed {
        step: ReplicationStep,
        detail: String,
    },
}

/// A step's answer: 200 passes; anything else fails it, in a few words.
fn passed(step: ReplicationStep, answer: Answer) -> Result<(), WarehouseOutcome> {
    let fail = |detail: String| WarehouseOutcome::Failed { step, detail };
    match answer {
        Ok((200, _)) => Ok(()),
        Ok((status, text)) => {
            let code = code(&text);
            Err(fail(format!("HTTP {status} {code}").trim_end().to_string()))
        }
        Err(detail) => Err(fail(detail)),
    }
}

impl RustfsLakeAdmin {
    async fn make(
        &self,
        source: &str,
        warehouse: &str,
        keys: &auth_gateway::LakeAdminKeys,
    ) -> Result<WarehouseOutcome, WarehouseOutcome> {
        use ReplicationStep::{WarehouseBucket, WarehouseTables};
        let base = self.base(source);
        let bucket = format!("/{warehouse}");
        let head = self.send(keys, call(&base, "HEAD", bucket.clone(), &[]));
        let made = match head.await {
            Ok((404, _)) => {
                let put = self.send(keys, call(&base, "PUT", bucket, &[])).await;
                passed(WarehouseBucket, put)?;
                true
            }
            other => {
                passed(WarehouseBucket, other)?;
                false
            }
        };
        let tables = format!("/iceberg/v1/buckets/{warehouse}");
        let put = self.send(keys, call(&base, "PUT", tables, &[])).await;
        passed(WarehouseTables, put)?;
        Ok(if made {
            WarehouseOutcome::Made
        } else {
            WarehouseOutcome::Present
        })
    }
}

impl RustfsLakeAdmin {
    /// Makes the warehouse on the main lake at `source` when it is missing,
    /// the bucket and its Iceberg table bucket; a second run makes nothing
    /// and asks for the table bucket again.
    pub async fn ensure(
        &self,
        source: &str,
        warehouse: &str,
    ) -> Result<WarehouseOutcome, RepositoryError> {
        let Some(keys) = self.keys.open(&Lake::main()).await? else {
            return Ok(WarehouseOutcome::NoKeys);
        };
        Ok(self
            .make(source, warehouse, &keys)
            .await
            .unwrap_or_else(|o| o))
    }
}
