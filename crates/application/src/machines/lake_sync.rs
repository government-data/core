//! Whether the lake's replica is in sync (decision 0078, the amendment "the
//! agent asks whether the lake's replica is in sync", ruled by the user
//! 2026-11-24): what the update agent asks before it updates a module whose
//! critical data the replica backs up. In sync: one replica approved and
//! answering; the main lake and the replica answering with no drive
//! offline; every warehouse's replication configured with nothing waiting
//! and nothing failed in the last hour. A read: nothing is changed or
//! audited, and the answer names no warehouse, bucket, endpoint, key or
//! count.

use auth_gateway::Lake;

use super::MachinesService;
use crate::lake::{LakeError, LakeHealth, LakeProbe, ReplicationState, ReplicationSync};

/// The capability whose data the core replicates.
pub const REPLICATED: &str = "lake";

/// Whether a replica holds what the main lake holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicaSync {
    InSync,
    /// Something waits to be copied.
    Behind,
    /// A replication is not configured, failed, or failed to copy in the
    /// last hour.
    Failed,
    /// No single replica is approved.
    NoReplica,
    /// A lake does not answer, or has a drive offline.
    Unhealthy,
    /// The core cannot say: the module provides nothing it replicates, or
    /// the lake's adapter does not say what waits and what failed.
    Unknown,
}

impl ReplicaSync {
    /// As `updater/v1` answers it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InSync => "in_sync",
            Self::Behind => "behind",
            Self::Failed => "failed",
            Self::NoReplica => "no_replica",
            Self::Unhealthy => "unhealthy",
            Self::Unknown => "unknown",
        }
    }
}

/// A lake answering with every drive online.
fn healthy(health: Result<LakeHealth, LakeError>) -> bool {
    matches!(
        LakeProbe::from(health),
        LakeProbe::Answered {
            drives_offline: 0,
            ..
        }
    )
}

/// One warehouse's replication; `None` while it is in sync.
fn warehouse(sync: Result<ReplicationSync, LakeError>) -> Option<ReplicaSync> {
    let sync = match sync {
        Ok(sync) => sync,
        Err(LakeError::Unknown(_)) => return Some(ReplicaSync::Failed),
        Err(_) => return Some(ReplicaSync::Unhealthy),
    };
    if sync.state != ReplicationState::Configured {
        return Some(ReplicaSync::Failed);
    }
    match (sync.pending, sync.failed) {
        (Some(_), Some(failed)) if failed > 0 => Some(ReplicaSync::Failed),
        (Some(pending), Some(_)) if pending > 0 => Some(ReplicaSync::Behind),
        (Some(_), Some(_)) => None,
        _ => Some(ReplicaSync::Unknown),
    }
}

/// The worst of the warehouses' states: failed, then unhealthy, behind,
/// unknown.
fn worst(states: impl IntoIterator<Item = ReplicaSync>) -> ReplicaSync {
    let rank = |s: &ReplicaSync| match s {
        ReplicaSync::Failed => 0,
        ReplicaSync::Unhealthy => 1,
        ReplicaSync::Behind => 2,
        ReplicaSync::Unknown => 3,
        _ => 4,
    };
    states
        .into_iter()
        .min_by_key(rank)
        .unwrap_or(ReplicaSync::InSync)
}

impl MachinesService {
    /// Whether the replica of the data of a module that provides
    /// `capabilities` is in sync now. A read.
    pub async fn replica_sync(&self, capabilities: &[String]) -> ReplicaSync {
        if !capabilities.iter().any(|c| c == REPLICATED) {
            return ReplicaSync::Unknown;
        }
        let (Some(admins), Some(main), Some(replication)) =
            (&self.lake_api, &self.main_lake, &self.replication)
        else {
            return ReplicaSync::Unhealthy;
        };
        let replica = match self.replica_machine().await {
            Ok(Ok(name)) => name,
            Ok(Err(_)) => return ReplicaSync::NoReplica,
            Err(_) => return ReplicaSync::Unhealthy,
        };
        let main_lake = admins.for_lake(&Lake::main(), main);
        let replica_lake = admins.for_lake(&Lake::Replica, &replica);
        if !healthy(main_lake.health().await) || !healthy(replica_lake.health().await) {
            return ReplicaSync::Unhealthy;
        }
        let Ok(warehouses) = replication.warehouses.warehouses().await else {
            return ReplicaSync::Unhealthy;
        };
        let mut states = Vec::new();
        for name in warehouses {
            if let Some(state) = warehouse(main_lake.replication(&name).await) {
                states.push(state);
            }
        }
        worst(states)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sync(state: ReplicationState, pending: Option<u64>, failed: Option<u64>) -> ReplicationSync {
        ReplicationSync {
            state,
            pending,
            failed,
        }
    }

    #[test]
    fn a_warehouse_is_in_sync_with_nothing_waiting_and_nothing_failed() {
        use ReplicationState::{Configured, NotConfigured};
        assert_eq!(warehouse(Ok(sync(Configured, Some(0), Some(0)))), None);
        let behind = warehouse(Ok(sync(Configured, Some(2), Some(0))));
        assert_eq!(behind, Some(ReplicaSync::Behind));
        let failed = warehouse(Ok(sync(Configured, Some(2), Some(1))));
        assert_eq!(failed, Some(ReplicaSync::Failed), "a failure counts first");
        let off = warehouse(Ok(sync(NotConfigured, Some(0), Some(0))));
        assert_eq!(off, Some(ReplicaSync::Failed));
        let older = warehouse(Ok(sync(Configured, None, None)));
        assert_eq!(
            older,
            Some(ReplicaSync::Unknown),
            "an adapter that does not say"
        );
        let missing = warehouse(Err(LakeError::Unknown("w".into())));
        assert_eq!(missing, Some(ReplicaSync::Failed));
        let down = warehouse(Err(LakeError::Unavailable("w".into())));
        assert_eq!(down, Some(ReplicaSync::Unhealthy));
    }

    #[test]
    fn the_worst_warehouse_says_the_lakes_state() {
        use ReplicaSync::*;
        assert_eq!(worst([]), InSync);
        assert_eq!(worst([Unknown, Behind]), Behind);
        assert_eq!(worst([Behind, Failed, Unhealthy]), Failed);
        assert_eq!(ReplicaSync::NoReplica.as_str(), "no_replica");
    }
}
