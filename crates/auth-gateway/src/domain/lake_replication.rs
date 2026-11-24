//! The replication of the main lake into the lake replica (decisions 0027,
//! 0028 and 0029, point 7; 0066 step 11, the user's rulings of 2026-11-13):
//! for each tenant's warehouse (`qd-tenant-*`), the replica's bucket with
//! object lock and a default retention, versioning and a lifecycle on the
//! source, the remote target through the source's replication proxy, and
//! the rule, without delete markers; before it, the warehouse itself on the
//! main lake when it is missing (the user's ruling of that day on the
//! warehouse). Configured by the application with the lakes' platform
//! admin keys, which only the adapter reads; each run is idempotent
//! (verified with RustFS 1.0.5).

use std::time::SystemTime;

use crate::domain::{Lake, RepositoryError};

/// The replica's default retention, `COMPLIANCE`, in days (ruling 4: a
/// constant of the decision, not a setting). No one can shorten it once an
/// object is written.
pub const REPLICA_RETENTION_DAYS: u32 = 30;

/// After how many days the source's noncurrent versions expire (ruling 5).
pub const SOURCE_NONCURRENT_DAYS: u32 = 7;

/// The port of the source's replication proxy, on the source node's
/// loopback (`infra/rustfs/replication.yml`).
pub const REPLICATION_PROXY_PORT: u16 = 9100;

/// A step of one warehouse's configuration, in the order they are done:
/// first the warehouse itself on the main lake, then its replication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicationStep {
    /// The warehouse's bucket on the main lake, made when missing.
    WarehouseBucket,
    /// Its Iceberg table bucket (`PUT /iceberg/v1/buckets/<warehouse>`).
    WarehouseTables,
    SourceBucket,
    ReplicaBucket,
    ReplicaRetention,
    SourceVersioning,
    SourceLifecycle,
    RemoteTarget,
    Rule,
}

impl ReplicationStep {
    /// Every step, in the order they are done.
    pub const ALL: [Self; 9] = [
        Self::WarehouseBucket,
        Self::WarehouseTables,
        Self::SourceBucket,
        Self::ReplicaBucket,
        Self::ReplicaRetention,
        Self::SourceVersioning,
        Self::SourceLifecycle,
        Self::RemoteTarget,
        Self::Rule,
    ];

    /// The step of that name ([`Self::as_str`]), if it is one.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == name)
    }

    /// As the API and the state name it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WarehouseBucket => "warehouse_bucket",
            Self::WarehouseTables => "warehouse_tables",
            Self::SourceBucket => "source_bucket",
            Self::ReplicaBucket => "replica_bucket",
            Self::ReplicaRetention => "replica_retention",
            Self::SourceVersioning => "source_versioning",
            Self::SourceLifecycle => "source_lifecycle",
            Self::RemoteTarget => "remote_target",
            Self::Rule => "rule",
        }
    }
}

/// What configuring one warehouse came to. Holds no key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplicationOutcome {
    /// Every step answered as expected (again, the same, on a second run).
    Configured,
    /// The source has no such bucket yet: nothing was done.
    NoWarehouse,
    /// The warehouse is on the main lake; its replication waits for the
    /// replica or its keys (what the readiness says).
    Made,
    /// `lake`'s admin keys are not kept: nothing was asked of it.
    NoKeys { lake: Lake },
    /// A step was refused or not reached: which, and what happened, in a
    /// few words, without a key.
    Failed {
        step: ReplicationStep,
        detail: String,
    },
}

/// One warehouse's replication as the last run left it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WarehouseState {
    pub warehouse: String,
    pub outcome: ReplicationOutcome,
    /// When it was first found configured; kept through later failures.
    pub configured_at: Option<SystemTime>,
    /// When it was configured or tried last.
    pub checked_at: SystemTime,
}

/// Port: what the last run left of each warehouse.
#[async_trait::async_trait]
pub trait ReplicationStateStore: Send + Sync {
    /// Keeps `outcome` for `warehouse` at `at`, in place of the last one.
    async fn record(
        &self,
        warehouse: &str,
        outcome: &ReplicationOutcome,
        at: SystemTime,
    ) -> Result<WarehouseState, RepositoryError>;

    /// Every warehouse's, by name.
    async fn all(&self) -> Result<Vec<WarehouseState>, RepositoryError>;
}

/// Port: the warehouses of the tenants this installation serves
/// (`qd-tenant-<id>`), whether or not the lake has them yet.
#[async_trait::async_trait]
pub trait Warehouses: Send + Sync {
    async fn warehouses(&self) -> Result<Vec<String>, RepositoryError>;
}
