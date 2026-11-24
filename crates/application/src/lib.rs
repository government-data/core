//! Application layer — use cases that orchestrate the domain.
//!
//! Depends only on `domain`. It receives domain ports (as trait objects or
//! generics) and exposes application services the delivery layer (`api`) calls.
//! It has no knowledge of HTTP, databases, or any framework.

use std::sync::Arc;

use domain::{Health, HealthCheck};

pub mod actors;
pub mod admins;
pub mod audit_chain;
pub mod lake;
pub mod machines;
pub mod platform;
pub mod settings;
pub mod updates;

pub use admins::{AdminContacts, PlatformAdmins};
pub use machines::{
    Acceptance, DoltHold, DoltHoldError, DoltStore, InstallError, InstallFileError, Installed,
    IssuedJoinToken, JoinCommand, JoinError, Joined, LakeAdminError, LakeCheckError, MachinesError,
    MachinesService, MainLakeState, Origin, RenewError, Renewed, ReplicaState, DOLT_HOLD_TTL,
    JOIN_TOKEN_TTL, LAKE_CHECK_TTL, LAKE_GATE_ROLES, LAKE_ROLES,
};
pub use machines::{AlertsApplied, AlertsError, DeliveredAlert, MachineMetrics};
pub use machines::{Drives, DrivesError, MAX_DRIVES};
pub use machines::{Lakes, ReplicationReadiness, ReplicationStatus, RECONCILER};
pub use machines::{PoolError, PoolNodes};
pub use platform::{
    CreatedServiceAccount, GroupChange, MintedToken, PlatformError, PlatformService, PolicyView,
    TokenMint,
};
pub use settings::{
    entra_own_tenant, DirectoryMember, DirectoryProbe, DirectoryProbeUser, EnvSettings,
    LdapSettings, MailSettings, MailSlot, MailTransport, Mailer, OAuthProviderSettings,
    OAuthSettings, OidcDiscovery, OutgoingMail, PlatformSettings, PlatformTimezone, SecretCipher,
    Section, SettingsConnectors, SettingsError, SettingsService, SettingsSource, SettingsStore,
    SettingsTargets, SmtpSecurity, StoredSection,
};
pub use updates::{
    DrainStatus, HistoryEntry, InLanguages, InstalledModule, Maintenance, MaintenanceError,
    ModuleAction, ModuleMenu, ModulePermissions, ModulePolicy, ModuleWork, ModulesWork, Requester,
    UpdatePolicy, UpdateRequest, UpdaterRow, UpdatesError, UpdatesService, UpdatesStore,
    UpdatesView,
};

/// Application service exposing the health use case.
///
/// Constructed from any adapter implementing the [`HealthCheck`] domain port,
/// so the composition root decides which concrete adapter is injected.
#[derive(Clone)]
pub struct HealthService {
    check: Arc<dyn HealthCheck>,
}

// `dyn HealthCheck` is not `Debug`, so derive can't apply; the workspace lint
// `missing_debug_implementations` still wants a `Debug` impl on this public
// type, so provide an opaque one.
impl std::fmt::Debug for HealthService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HealthService").finish_non_exhaustive()
    }
}

impl HealthService {
    /// Build the service from an adapter implementing the domain port.
    pub fn new(check: Arc<dyn HealthCheck>) -> Self {
        Self { check }
    }

    /// Report the current health of the service.
    ///
    /// `async` because the underlying port probes an I/O-bound dependency; the
    /// service just forwards to the injected adapter.
    pub async fn health(&self) -> Health {
        self.check.check().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{Health, Readiness};

    struct StubCheck(Readiness);

    #[async_trait::async_trait]
    impl HealthCheck for StubCheck {
        async fn check(&self) -> Health {
            Health { readiness: self.0 }
        }
    }

    #[tokio::test]
    async fn health_service_reports_the_ports_readiness() {
        let service = HealthService::new(Arc::new(StubCheck(Readiness::Ready)));
        assert_eq!(service.health().await.readiness, Readiness::Ready);

        let service = HealthService::new(Arc::new(StubCheck(Readiness::NotReady)));
        assert_eq!(service.health().await.readiness, Readiness::NotReady);
    }
}
