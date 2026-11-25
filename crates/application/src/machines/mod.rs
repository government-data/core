//! The installation's machines (decisions 0029 and 0066): join tokens, the
//! pending → approved → draining → retired flow, and the serial of the
//! certificate each one uses. Every issue, use and refusal is audited; the
//! token, and its hash, never are.
//!
//! The model and the ports are `auth_gateway`'s (`MachineRegistry`,
//! `JoinTokenMint`): `infrastructure` implements them, and it cannot see this
//! crate.

mod alerts;
mod audit;
mod calling;
mod dolt;
mod drives;
mod install;
mod install_files;
mod join;
mod lake;
mod lake_admin;
mod lake_keys;
mod lake_main;
mod lake_pool_install;
mod lake_pools;
mod lake_replica;
mod lake_replication;
mod lake_sync;
mod lake_warehouse;
mod lifecycle;
mod registry;
mod renew;
mod series;

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use auth_gateway::domain::machine::is_label;
use auth_gateway::{
    is_join_token, CertificateSigner, JoinTokenMint, Machine, MachineRegistry, MachineRole,
    MachineSessions, MachineState, NewJoinToken, RedeemRefusal,
};
use domain::{AdminId, AuditRepository, Clock, RepositoryError};

pub use alerts::{AlertsApplied, AlertsError, DeliveredAlert};
pub use audit::Origin;
pub use calling::Acceptance;
pub use dolt::{DoltHold, DoltHoldError, DoltStore, DOLT_HOLD_TTL};
pub use drives::{Drives, DrivesError, MAX_DRIVES};
pub use install::{InstallError, Installed};
pub use install_files::{InstallFileError, JoinCommand};
pub use join::Joined;
pub use lake::{LakeCheckError, LAKE_CHECK_TTL, LAKE_GATE_ROLES, LAKE_ROLES};
pub use lake_admin::LakeAdminError;
pub use lake_main::MainLakeState;
pub use lake_pools::{PoolError, PoolNodes};
pub use lake_replica::ReplicaState;
pub use lake_replication::{Lakes, ReplicationReadiness, ReplicationStatus, RECONCILER};
pub use lake_sync::{ReplicaSync, REPLICATED};
use lifecycle::done;
pub use registry::RegistryOutcome;
pub use renew::{RenewError, Renewed};
pub use series::MachineMetrics;

/// How long a join token can be used (0029).
pub const JOIN_TOKEN_TTL: Duration = Duration::from_secs(3600);

/// A join token, shown once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedJoinToken {
    pub token: String,
    pub role: MachineRole,
    pub pool: Option<String>,
    /// The lake pool's node it is for, which the command names.
    pub node: Option<String>,
    pub expires_at: SystemTime,
}

/// Why a machine could not join.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinError {
    /// The token is unknown, used or expired: one answer for the three.
    Refused,
    /// The name is not lowercase letters, digits and hyphens, 1 to 63.
    InvalidName,
    /// A machine that is not retired has the name.
    NameTaken,
    /// The token is a lake pool node's and the name is not that node's.
    NodeName,
    /// The signing request is not one the CA signs, or the name cannot be
    /// a certificate's.
    InvalidRequest,
    /// The installation has no CA: nothing can join.
    Unavailable,
    Backend(String),
}

/// Why a change to machines did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachinesError {
    /// A pool or a serial out of form.
    Invalid(&'static str),
    NotFound,
    /// Not allowed from the state the machine is in.
    NotAllowed(MachineState),
    /// The installation has no CA: a join token could not be redeemed.
    Unavailable,
    Backend(String),
}

impl From<RepositoryError> for MachinesError {
    fn from(e: RepositoryError) -> Self {
        Self::Backend(e.to_string())
    }
}

impl From<RepositoryError> for JoinError {
    fn from(e: RepositoryError) -> Self {
        Self::Backend(e.to_string())
    }
}

/// The machines' use cases.
#[derive(Clone)]
pub struct MachinesService {
    registry: Arc<dyn MachineRegistry>,
    mint: Arc<dyn JoinTokenMint>,
    audit: Arc<dyn AuditRepository>,
    clock: Arc<dyn Clock>,
    /// The installation's CA; without it no token is issued and none joins.
    signer: Option<Arc<dyn CertificateSigner>>,
    /// The machines' sessions on the queue's PostgreSQL, ended at retiring.
    sessions: Option<Arc<dyn MachineSessions>>,
    /// The catalog's Dolt, for its restart.
    dolt: Option<Arc<dyn DoltStore>>,
    /// The lake's gate's passes, shared by every clone.
    lake: Arc<lake::LakePasses>,
    /// The passes of the gates themselves, as askers.
    lake_gates: Arc<lake::LakePasses>,
    /// The files a new machine is installed with, and where it is.
    install: Option<install_files::Install>,
    /// The lakes' root keys, given to their nodes against a join token.
    lake_keys: Option<Arc<dyn auth_gateway::LakeRootKeyStore>>,
    /// The lakes' admin keys, as a person pasted them.
    lake_admin: Option<Arc<dyn auth_gateway::LakeAdminKeyStore>>,
    /// The lakes' admin API, signed with those keys.
    lake_api: Option<Arc<dyn crate::lake::LakeAdmins>>,
    /// Where the main lake answers (`LAKE_ENDPOINT`), for its admin API.
    main_lake: Option<String>,
    /// The replication into the replica: its adapter, state and warehouses.
    replication: Option<lake_replication::Replication>,
    /// The lake's pools, as a person defined them.
    pools: Option<Arc<dyn auth_gateway::LakePoolStore>>,
    /// The machines' active alerts, as Grafana delivers them.
    alerts: Option<Arc<dyn auth_gateway::MachineAlertStore>>,
    /// The machines' series, read from Mimir for their charts.
    series: Option<Arc<dyn auth_gateway::MachineSeries>>,
}

impl std::fmt::Debug for MachinesService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MachinesService").finish_non_exhaustive()
    }
}

impl MachinesService {
    pub fn new(
        registry: Arc<dyn MachineRegistry>,
        mint: Arc<dyn JoinTokenMint>,
        audit: Arc<dyn AuditRepository>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            registry,
            mint,
            audit,
            clock,
            signer: None,
            sessions: None,
            dolt: None,
            lake: Arc::default(),
            lake_gates: Arc::default(),
            install: None,
            lake_keys: None,
            lake_admin: None,
            lake_api: None,
            main_lake: None,
            replication: None,
            pools: None,
            alerts: None,
            series: None,
        }
    }

    /// With the machines' sessions on the queue, which retiring ends.
    pub fn with_sessions(mut self, sessions: Arc<dyn MachineSessions>) -> Self {
        self.sessions = Some(sessions);
        self
    }

    /// With the installation's CA, which signs the certificates of the
    /// machines that join.
    pub fn with_signer(mut self, signer: Arc<dyn CertificateSigner>) -> Self {
        self.signer = Some(signer);
        self
    }

    /// Whether machines can join: the installation has a CA.
    pub fn can_join(&self) -> bool {
        self.signer.is_some()
    }

    /// A token for one machine of `role` (and `pool`) to join, for an hour;
    /// never for the application VM's role, which does not join, nor for a
    /// lake pool's node, whose token names it ([`Self::issue_node_token`]).
    /// Only its hash is stored; the token is returned here and never again.
    /// Refused without a CA, as such a token could not be redeemed.
    pub async fn issue_join_token(
        &self,
        role: MachineRole,
        pool: Option<&str>,
        by: &AdminId,
        origin: &Origin,
    ) -> Result<IssuedJoinToken, MachinesError> {
        if role == MachineRole::Lake {
            return Err(MachinesError::Invalid("node"));
        }
        self.issue(role, pool, None, by, origin).await
    }

    async fn issue(
        &self,
        role: MachineRole,
        pool: Option<&str>,
        node: Option<&str>,
        by: &AdminId,
        origin: &Origin,
    ) -> Result<IssuedJoinToken, MachinesError> {
        if !self.can_join() {
            return Err(MachinesError::Unavailable);
        }
        if !role.joins() {
            return Err(MachinesError::Invalid("role"));
        }
        if pool.is_some_and(|p| !is_label(p)) {
            return Err(MachinesError::Invalid("pool"));
        }
        let token = self
            .mint
            .mint()
            .ok_or_else(|| MachinesError::Backend("no random bytes for a join token".into()))?;
        let now = self.clock.now();
        let expires_at = now + JOIN_TOKEN_TTL;
        let pool = pool.map(str::to_string);
        let node = node.map(str::to_string);
        let id = self
            .registry
            .issue_token(&NewJoinToken {
                token_hash: self.mint.hash(&token),
                role,
                pool: pool.clone(),
                node: node.clone(),
                created_by: by.clone(),
                expires_at,
            })
            .await?;
        self.record(audit::token_issued(
            &id,
            role,
            pool.as_deref(),
            node.as_deref(),
            by,
            origin,
            now,
        ))
        .await?;
        Ok(IssuedJoinToken {
            token,
            role,
            pool,
            node,
            expires_at,
        })
    }

    /// A machine joins with its token: it is made `pending` with the
    /// token's role and pool, and the token is used up. Used, unknown and
    /// expired tokens get the same [`JoinError::Refused`].
    pub async fn redeem(
        &self,
        token: &str,
        machine_name: &str,
        origin: &Origin,
    ) -> Result<Machine, JoinError> {
        let now = self.clock.now();
        let refuse = |reason, error| self.refuse(machine_name, reason, error, origin, now);
        if !is_label(machine_name) {
            return Err(refuse("invalid-name", JoinError::InvalidName).await);
        }
        if !is_join_token(token) {
            return Err(refuse("token", JoinError::Refused).await);
        }
        let hash = self.mint.hash(token);
        match self.registry.redeem(&hash, machine_name, now).await? {
            Ok(machine) => {
                self.record(audit::join_used(&machine, origin, now)).await?;
                Ok(machine)
            }
            Err(RedeemRefusal::Token) => Err(refuse("token", JoinError::Refused).await),
            Err(RedeemRefusal::NameTaken) => Err(refuse("name-taken", JoinError::NameTaken).await),
            Err(RedeemRefusal::NodeName) => Err(refuse("node-name", JoinError::NodeName).await),
        }
    }

    /// A refusal is audited, and answered whether or not it could be.
    async fn refuse(
        &self,
        machine_name: &str,
        reason: &'static str,
        error: JoinError,
        origin: &Origin,
        now: SystemTime,
    ) -> JoinError {
        let event = audit::join_refused(machine_name, reason, origin, now);
        if let Err(e) = self.record(event).await {
            tracing::error!(error = %e, "a refused join could not be audited");
        }
        error
    }

    /// Every machine, for Settings → Execution.
    pub async fn list(&self) -> Result<Vec<Machine>, MachinesError> {
        Ok(self.registry.list().await?)
    }

    async fn record(&self, event: domain::NewActionAudit) -> Result<(), RepositoryError> {
        self.audit.record_action(&event).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests;
