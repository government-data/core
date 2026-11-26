//! Each lake's adapter, faked (decision 0078, step 10): it records what it
//! was asked, by the lake and where the core found it, answers its health,
//! makes a missing warehouse and keeps it, and answers the outcomes a test
//! sets per warehouse.

use std::sync::Arc;

use crate::lake::{
    LakeAdmin, LakeAdmins, LakeCredentialKind, LakeError, LakeGrant, LakeHealth, LakeKeys,
    ManagedPolicy, ReplicaTarget, ReplicationState, ReplicationSync, RetentionRules, SessionPolicy,
    WarehouseMade,
};

use super::*;

/// What was asked of a lake's adapter: by the lake (its key) and its node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Asked {
    Health(String, String),
    Warehouse(String, String, String),
    Replicate(String, String, String, ReplicaTarget),
    /// The lake, the kind, and the access key held.
    Credentials(String, LakeCredentialKind, Option<String>),
}

#[derive(Default)]
pub(super) struct Adapters {
    pub(super) health: Mutex<Option<LakeHealth>>,
    pub(super) made: Mutex<Vec<(String, Result<WarehouseMade, LakeError>)>>,
    pub(super) replicated: Mutex<Vec<(String, Result<ReplicationState, LakeError>)>>,
    pub(super) asked: Mutex<Vec<Asked>>,
    /// What the replication's read answers; `None` is "not asked here".
    pub(super) synced: Mutex<Option<ReplicationSync>>,
    there: Mutex<Vec<String>>,
}

impl Adapters {
    /// The warehouses asked to be made, as `(node, warehouse)`.
    pub(super) fn warehouses_asked(&self) -> Vec<(String, String)> {
        let asked = self.asked.lock().unwrap();
        let made = asked.iter().filter_map(|a| match a {
            Asked::Warehouse(_, node, w) => Some((node.clone(), w.clone())),
            _ => None,
        });
        made.collect()
    }

    /// The replications asked, as `(node, warehouse, target)`.
    pub(super) fn replications_asked(&self) -> Vec<(String, String, ReplicaTarget)> {
        let asked = self.asked.lock().unwrap();
        let replicated = asked.iter().filter_map(|a| match a {
            Asked::Replicate(_, node, w, t) => Some((node.clone(), w.clone(), t.clone())),
            _ => None,
        });
        replicated.collect()
    }
}

/// [`Adapters`] as the port.
pub(super) struct Shared(pub(super) Arc<Adapters>);

impl LakeAdmins for Shared {
    fn for_lake(&self, lake: &auth_gateway::Lake, node: &str) -> Arc<dyn LakeAdmin> {
        Arc::new(At {
            all: self.0.clone(),
            lake: lake.key(),
            node: node.into(),
        })
    }
}

struct At {
    all: Arc<Adapters>,
    lake: String,
    node: String,
}

fn not_here<T>() -> Result<T, LakeError> {
    Err(LakeError::Unknown("not asked in these tests".into()))
}

#[async_trait]
impl LakeAdmin for At {
    async fn ensure_warehouse(
        &self,
        name: &str,
        _: &RetentionRules,
    ) -> Result<WarehouseMade, LakeError> {
        let asked = Asked::Warehouse(self.lake.clone(), self.node.clone(), name.into());
        self.all.asked.lock().unwrap().push(asked);
        let made = self.all.made.lock().unwrap();
        if let Some((_, answer)) = made.iter().find(|(w, _)| w == name) {
            return answer.clone();
        }
        let mut there = self.all.there.lock().unwrap();
        if there.iter().any(|w| w == name) {
            return Ok(WarehouseMade::Present);
        }
        there.push(name.to_string());
        Ok(WarehouseMade::Made)
    }

    async fn replicate(
        &self,
        name: &str,
        replica: &ReplicaTarget,
    ) -> Result<ReplicationState, LakeError> {
        let (lake, node) = (self.lake.clone(), self.node.clone());
        let asked = Asked::Replicate(lake, node, name.into(), replica.clone());
        self.all.asked.lock().unwrap().push(asked);
        let replicated = self.all.replicated.lock().unwrap();
        let found = replicated.iter().find(|(w, _)| w == name);
        found.map_or(Ok(ReplicationState::Configured), |(_, a)| a.clone())
    }

    async fn replication(&self, _: &str) -> Result<ReplicationSync, LakeError> {
        let synced = self.all.synced.lock().unwrap().clone();
        synced.map_or_else(not_here, Ok)
    }

    async fn set_managed_policies(&self, _: &[ManagedPolicy]) -> Result<(), LakeError> {
        not_here()
    }

    async fn session_policy(&self, _: &LakeGrant) -> Result<SessionPolicy, LakeError> {
        not_here()
    }

    async fn ensure_credentials(
        &self,
        kind: LakeCredentialKind,
        held: Option<&LakeKeys>,
    ) -> Result<LakeKeys, LakeError> {
        let access = held.map(|k| k.access_key.clone());
        let asked = Asked::Credentials(self.lake.clone(), kind, access);
        self.all.asked.lock().unwrap().push(asked);
        held.cloned()
            .ok_or_else(|| LakeError::Invalid("no keys held".into()))
    }

    async fn health(&self) -> Result<LakeHealth, LakeError> {
        let asked = Asked::Health(self.lake.clone(), self.node.clone());
        self.all.asked.lock().unwrap().push(asked);
        let health = self.all.health.lock().unwrap().clone();
        health.ok_or_else(|| LakeError::Unavailable("GET health: no connection".into()))
    }
}
