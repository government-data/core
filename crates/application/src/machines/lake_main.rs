//! The main lake as Settings → Lake shows it (decision 0028, "Lake
//! health"; 0066 step 11, the user's ruling 1 of 2026-11-13): today's
//! single RustFS, the replication's source until a pool is installed, its
//! admin keys kept as `pool:main`. It is not a machine of the registry: it
//! is reached where the application reaches the lake (`LAKE_ENDPOINT`),
//! through its gate. The keys are read by the core's client of the lake's
//! adapter alone (decision 0078).

use auth_gateway::Lake;

use crate::lake::LakeProbe;

use super::{LakeAdminError, MachinesService};

/// What Settings → Lake says of the main lake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MainLakeState {
    /// The application has no address for the lake: nothing was asked.
    NoAddress,
    /// The lake at `endpoint`, and what it answered.
    Probed { endpoint: String, probe: LakeProbe },
}

impl MachinesService {
    /// With where the main lake answers (`LAKE_ENDPOINT`, its gate's base
    /// URL), if the application reaches one.
    pub fn with_main_lake(mut self, endpoint: Option<String>) -> Self {
        self.main_lake = endpoint
            .map(|e| e.trim().trim_end_matches('/').to_string())
            .filter(|e| !e.is_empty());
        self
    }

    /// Where the main lake answers, if the application reaches one.
    pub fn main_lake(&self) -> Option<&str> {
        self.main_lake.as_deref()
    }

    /// What the main lake's admin API answers now with the kept keys. A
    /// read: nothing is changed or audited.
    pub async fn main_lake_state(&self) -> Result<MainLakeState, LakeAdminError> {
        let api = self.lake_api.as_ref().ok_or(LakeAdminError::Unavailable)?;
        let Some(endpoint) = self.main_lake.clone() else {
            return Ok(MainLakeState::NoAddress);
        };
        let probe = LakeProbe::from(api.for_lake(&Lake::main(), &endpoint).health().await);
        Ok(MainLakeState::Probed { endpoint, probe })
    }
}
