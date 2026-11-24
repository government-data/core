//! `application::lake::LakeAdmins` over [`RustfsLakeAdmin`] (decision 0078,
//! step 10): a bridge that keeps the server asking RustFS's admin API in
//! process until it takes the generic client of `lake-admin/1` (step 11),
//! which removes it. It serves what the machines' use cases ask: a lake's
//! health, a warehouse, its replication.

use std::sync::Arc;

use application::lake::{
    LakeAdmin, LakeAdmins, LakeCredentialKind, LakeError, LakeGrant, LakeHealth, LakeKeys,
    LakeProbe, ManagedPolicy, ReplicaTarget, ReplicationState, RetentionRules, SessionPolicy,
    WarehouseMade,
};
use async_trait::async_trait;
use auth_gateway::{Lake, ReplicationOutcome};

use crate::rustfs_admin::RustfsLakeAdmin;
use crate::rustfs_replication::{proxy_of, WarehouseReplication};
use crate::rustfs_warehouse::WarehouseOutcome;

impl LakeAdmins for RustfsLakeAdmin {
    fn for_lake(&self, lake: &Lake, node: &str) -> Arc<dyn LakeAdmin> {
        Arc::new(At {
            api: self.clone(),
            lake: lake.clone(),
            node: node.to_string(),
        })
    }
}

/// One lake, where the core finds it.
struct At {
    api: RustfsLakeAdmin,
    lake: Lake,
    node: String,
}

fn backend(e: domain::RepositoryError) -> LakeError {
    LakeError::Unavailable(e.to_string())
}

fn not_served<T>(what: &str) -> Result<T, LakeError> {
    Err(LakeError::Unknown(format!("{what}: not served in process")))
}

/// A replication's outcome as the port says it.
fn state(outcome: ReplicationOutcome) -> Result<ReplicationState, LakeError> {
    match outcome {
        ReplicationOutcome::Configured => Ok(ReplicationState::Configured),
        ReplicationOutcome::Failed { step, detail } => Ok(ReplicationState::Failed {
            step: step.as_str().into(),
            detail,
        }),
        ReplicationOutcome::NoWarehouse => Err(LakeError::Unknown("no such warehouse".into())),
        ReplicationOutcome::NoKeys { lake } => Err(LakeError::Invalid(format!(
            "no admin keys kept for {}",
            lake.key()
        ))),
        ReplicationOutcome::Made => Ok(ReplicationState::NotConfigured),
    }
}

#[async_trait]
impl LakeAdmin for At {
    async fn ensure_warehouse(
        &self,
        name: &str,
        _: &RetentionRules,
    ) -> Result<WarehouseMade, LakeError> {
        match self.api.ensure(&self.node, name).await.map_err(backend)? {
            WarehouseOutcome::Made => Ok(WarehouseMade::Made),
            WarehouseOutcome::Present => Ok(WarehouseMade::Present),
            WarehouseOutcome::Failed { step, detail } => Ok(WarehouseMade::Failed {
                step: step.as_str().into(),
                detail,
            }),
            WarehouseOutcome::NoKeys => Err(LakeError::Invalid(format!(
                "no admin keys kept for {}",
                self.lake.key()
            ))),
        }
    }

    async fn replicate(
        &self,
        name: &str,
        replica: &ReplicaTarget,
    ) -> Result<ReplicationState, LakeError> {
        let Some(proxy) = proxy_of(&self.node, replica.source_pool) else {
            return Err(LakeError::Invalid(format!(
                "not a lake's address: {}",
                self.node
            )));
        };
        let plan = WarehouseReplication {
            warehouse: name.to_string(),
            source: self.node.clone(),
            replica: replica.endpoint.clone(),
            proxy,
        };
        state(self.api.configure(&plan).await.map_err(backend)?)
    }

    async fn replication(&self, _: &str) -> Result<ReplicationState, LakeError> {
        not_served("GET replication")
    }

    async fn set_managed_policies(&self, _: &[ManagedPolicy]) -> Result<(), LakeError> {
        not_served("PUT policies/managed")
    }

    async fn session_policy(&self, _: &LakeGrant) -> Result<SessionPolicy, LakeError> {
        not_served("POST policies/session")
    }

    async fn ensure_credentials(
        &self,
        _: LakeCredentialKind,
        _: Option<&LakeKeys>,
    ) -> Result<LakeKeys, LakeError> {
        not_served("PUT credentials")
    }

    async fn health(&self) -> Result<LakeHealth, LakeError> {
        let probe = self.api.probe(&self.lake, &self.node).await;
        match probe.map_err(backend)? {
            LakeProbe::NoKeys => Ok(LakeHealth::NoKeys),
            LakeProbe::Answered {
                drives_online,
                drives_offline,
            } => Ok(LakeHealth::Answered {
                drives_online,
                drives_offline,
            }),
            LakeProbe::KeysRefused { code } => Ok(LakeHealth::KeysRefused { code }),
            LakeProbe::Unreachable { detail } => Err(LakeError::Lake(detail)),
        }
    }
}
