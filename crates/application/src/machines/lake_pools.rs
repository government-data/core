//! A lake's pools in Settings → Lake (decision 0029, points 5 and 6; 0066
//! step 11, the user's rulings 1 to 3 of 2026-11-14): a person who may
//! manage machines defines a pool before any of its nodes exists; each node
//! then gets a join token that names it, and only a machine of that name
//! joins with it.
//!
//! The first pool is `main` (ruling 3); adding another to a lake whose
//! nodes are running is 0029's guided rolling restart (point 6), not built,
//! so only `main` is taken. A pool is defined again in place while none of
//! its nodes has joined: once one has, its layout is the deployment's.

use std::sync::Arc;

use auth_gateway::{
    LakePool, LakePoolStore, Machine, MachineRole, MachineState, PoolLayout, PoolRefusal, MAIN_POOL,
};
use domain::{AdminId, AuditEventType, NewActionAudit, RepositoryError};

use super::{IssuedJoinToken, MachinesError, MachinesService, Origin};

/// Why a pool was not defined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolError {
    /// The layout given is out of form.
    Layout(PoolRefusal),
    /// Not `main`: a second pool is not built (0029, point 6).
    NotMain,
    /// A node of the pool has joined: its layout stays.
    Joined,
    /// The application has no store for pools.
    Unavailable,
    Backend(String),
}

impl From<RepositoryError> for PoolError {
    fn from(e: RepositoryError) -> Self {
        Self::Backend(e.to_string())
    }
}

/// A pool and each of its nodes, in order, with the machine that joined
/// as that node, if one has and is not retired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolNodes {
    pub pool: LakePool,
    pub nodes: Vec<(String, Option<Machine>)>,
}

/// The live machines that joined as nodes of `pool`.
fn joined<'a>(machines: &'a [Machine], pool: &'a str) -> impl Iterator<Item = &'a Machine> {
    machines.iter().filter(move |m| {
        m.role == MachineRole::Lake
            && m.pool.as_deref() == Some(pool)
            && m.state != MachineState::Retired
    })
}

fn view(pool: LakePool, machines: &[Machine]) -> PoolNodes {
    let nodes = pool
        .layout
        .nodes()
        .iter()
        .map(|node| {
            let name = pool.layout.name();
            let machine = joined(machines, name).find(|m| &m.name == node);
            (node.clone(), machine.cloned())
        })
        .collect();
    PoolNodes { pool, nodes }
}

impl MachinesService {
    /// With the store of the lake's pools.
    pub fn with_lake_pools(mut self, pools: Arc<dyn LakePoolStore>) -> Self {
        self.pools = Some(pools);
        self
    }

    /// Defines the pool `name` as `nodes` in order, each with `drives`
    /// drives, in place of its definition before while none of its nodes
    /// has joined; audited before it answers.
    pub async fn define_pool(
        &self,
        name: &str,
        nodes: &[String],
        drives: u16,
        by: &AdminId,
        origin: &Origin,
    ) -> Result<PoolNodes, PoolError> {
        let layout = PoolLayout::parse(name, nodes, drives).map_err(PoolError::Layout)?;
        if layout.name() != MAIN_POOL {
            return Err(PoolError::NotMain);
        }
        let store = self.pools.as_ref().ok_or(PoolError::Unavailable)?;
        let machines = self.registry.list().await?;
        if joined(&machines, layout.name()).next().is_some() {
            return Err(PoolError::Joined);
        }
        let now = self.clock.now();
        let pool = store.define(&layout, by, now).await?;
        self.record(defined(layout.name(), by, origin, now)).await?;
        Ok(view(pool, &machines))
    }

    /// Every pool, with its nodes and which have joined. A read.
    pub async fn lake_pools(&self) -> Result<Vec<PoolNodes>, PoolError> {
        let store = self.pools.as_ref().ok_or(PoolError::Unavailable)?;
        let machines = self.registry.list().await?;
        let pools = store.all().await?;
        Ok(pools.into_iter().map(|p| view(p, &machines)).collect())
    }

    /// A token for the node `node` of the pool `pool` to join, role `lake`:
    /// the pool must be defined and the node one of its own. `drives`, how
    /// many drives the command carries, is none or the pool's count.
    pub async fn issue_node_token(
        &self,
        pool: &str,
        node: &str,
        drives: usize,
        by: &AdminId,
        origin: &Origin,
    ) -> Result<IssuedJoinToken, MachinesError> {
        let store = self.pools.as_ref();
        let store = store.ok_or_else(|| MachinesError::Backend("no store for pools".into()))?;
        let Some(defined) = store.find(pool).await? else {
            return Err(MachinesError::Invalid("pool"));
        };
        if !defined.layout.nodes().iter().any(|n| n == node) {
            return Err(MachinesError::Invalid("node"));
        }
        if drives != 0 && drives != usize::from(defined.layout.drives()) {
            return Err(MachinesError::Invalid("drives"));
        }
        let role = MachineRole::Lake;
        self.issue(role, Some(pool), Some(node), by, origin).await
    }
}

/// `lake:pool:defined` on `lake/pool:<name>`, by the person.
fn defined(name: &str, by: &AdminId, origin: &Origin, at: std::time::SystemTime) -> NewActionAudit {
    NewActionAudit {
        event_type: AuditEventType::ResourceChanged,
        admin_id: Some(by.clone()),
        service_account: None,
        job_id: None,
        action: "lake:pool:defined".into(),
        resource: format!("lake/pool:{name}"),
        ip: origin.ip.clone(),
        user_agent: origin.user_agent.clone(),
        occurred_at: at,
    }
}
