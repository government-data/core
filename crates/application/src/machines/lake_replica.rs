//! The lake replica as Settings → Lake shows it (decision 0028, "Lake
//! health"; 0066 step 11): which machine it is, and what it answers, asked
//! with the admin keys a person pasted. The replica is the machine of role
//! `lake-replica` that is approved or draining, reached by its name as its
//! certificate carries it (the gate serves that certificate); no setting
//! names it. The keys are read by the lake's adapter alone: nothing here
//! holds the secret.

use std::sync::Arc;

use auth_gateway::Lake;

use crate::lake::{LakeAdmins, LakeProbe};

use super::{LakeAdminError, MachinesService};

/// What Settings → Lake says of the replica.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplicaState {
    /// No machine of role `lake-replica` is approved or draining; `pending`
    /// names one that joined and waits for a person to approve it.
    NoMachine { pending: Option<String> },
    /// More than one is approved or draining: which is the replica is not
    /// chosen here.
    SeveralMachines { names: Vec<String> },
    /// The replica, and what it answered.
    Probed { machine: String, probe: LakeProbe },
}

impl MachinesService {
    /// With each lake's adapter, whose client reads the kept keys itself.
    pub fn with_lake_admin(mut self, api: Arc<dyn LakeAdmins>) -> Self {
        self.lake_api = Some(api);
        self
    }

    /// The replica's machine, and what its admin API answers now with the
    /// kept keys. Nothing is changed or audited: it is a read.
    pub async fn replica_state(&self) -> Result<ReplicaState, LakeAdminError> {
        let api = self.lake_api.as_ref().ok_or(LakeAdminError::Unavailable)?;
        let machine = match self.replica_machine().await? {
            Ok(machine) => machine,
            Err(state) => return Ok(state),
        };
        let health = api.for_lake(&Lake::Replica, &machine).health().await;
        let probe = LakeProbe::from(health);
        Ok(ReplicaState::Probed { machine, probe })
    }
}
