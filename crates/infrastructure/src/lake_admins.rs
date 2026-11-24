//! Each lake's adapter, as the core reaches it (decision 0078, step 11, the
//! user's rulings of 2026-11-24): `application::lake::LakeAdmins` over the
//! generic client of `lake-admin/1`. The main lake's adapter is at
//! `LAKE_ADMIN_URL` (until the module's manifest declares it, step 17); a
//! replica's on its machine, on port 443, its certificate of role
//! `lake-replica`. Every call presents the application VM's certificate
//! (`LAKE_ADMIN_CERT_FILE`, by default `LAKE_CLIENT_CERT_FILE`) and trusts the installation's CA alone
//! (`LAKE_ADMIN_CA_FILE`, by default `CA_CERT_FILE`), both read again for
//! each lake asked, so a renewed certificate is presented without a
//! restart; the kept admin keys go with each call that needs them.

use std::path::PathBuf;
use std::sync::Arc;

use application::lake::{
    LakeAdmin, LakeAdmins, LakeCredentialKind, LakeError, LakeGrant, LakeHealth, LakeKeys,
    ManagedPolicy, ReplicaTarget, ReplicationState, RetentionRules, SessionPolicy, WarehouseMade,
};
use async_trait::async_trait;
use auth_gateway::Lake;

use crate::lake_admin_client::{AdminKeySource, LakeAdminClient};

/// Where a replica's adapter listens (the user's ruling of 2026-11-24).
pub const REPLICA_ADAPTER_PORT: u16 = 443;

/// The role of a replica's machines, which its adapter's certificate
/// carries.
pub const REPLICA_ROLE: &str = "lake-replica";

/// The CA and the certificate the core presents.
#[derive(Debug, Clone)]
enum Pems {
    Files { ca: PathBuf, identity: PathBuf },
    Given { ca: String, identity: String },
}

impl Pems {
    fn read(&self) -> Result<(String, String), String> {
        match self {
            Self::Files { ca, identity } => {
                let read = |p: &PathBuf| {
                    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
                };
                Ok((read(ca)?, read(identity)?))
            }
            Self::Given { ca, identity } => Ok((ca.clone(), identity.clone())),
        }
    }
}

/// The lakes' adapters.
#[derive(Clone)]
pub struct AdapterLakeAdmins {
    main: Option<String>,
    pems: Pems,
    keys: Arc<dyn AdminKeySource>,
    replica_port: u16,
}

impl std::fmt::Debug for AdapterLakeAdmins {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdapterLakeAdmins")
            .field("main", &self.main)
            .field("replica_port", &self.replica_port)
            .finish_non_exhaustive()
    }
}

impl AdapterLakeAdmins {
    /// From `LAKE_ADMIN_URL` (optional: without it the main lake's adapter
    /// is not reached), `LAKE_ADMIN_CA_FILE` or `CA_CERT_FILE`, and
    /// `LAKE_ADMIN_CERT_FILE` or `LAKE_CLIENT_CERT_FILE`.
    pub fn from_env(keys: Arc<dyn AdminKeySource>) -> Result<Self, String> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let ca = var("LAKE_ADMIN_CA_FILE")
            .or_else(|| var("CA_CERT_FILE"))
            .ok_or("neither LAKE_ADMIN_CA_FILE nor CA_CERT_FILE is set")?;
        let identity = var("LAKE_ADMIN_CERT_FILE")
            .or_else(|| var("LAKE_CLIENT_CERT_FILE"))
            .ok_or("neither LAKE_ADMIN_CERT_FILE nor LAKE_CLIENT_CERT_FILE is set")?;
        let pems = Pems::Files {
            ca: ca.into(),
            identity: identity.into(),
        };
        Ok(Self::with(var("LAKE_ADMIN_URL"), pems, keys))
    }

    /// The main lake's adapter at `main`, with the CA and the core's
    /// certificate given whole (tests).
    pub fn new(
        main: Option<&str>,
        ca_pem: &str,
        identity_pem: &str,
        keys: Arc<dyn AdminKeySource>,
    ) -> Self {
        let pems = Pems::Given {
            ca: ca_pem.into(),
            identity: identity_pem.into(),
        };
        Self::with(main.map(str::to_string), pems, keys)
    }

    fn with(main: Option<String>, pems: Pems, keys: Arc<dyn AdminKeySource>) -> Self {
        Self {
            main,
            pems,
            keys,
            replica_port: REPLICA_ADAPTER_PORT,
        }
    }

    /// A replica's adapter on `port` instead (tests).
    pub fn replica_port(mut self, port: u16) -> Self {
        self.replica_port = port;
        self
    }

    fn client(&self, lake: &Lake, node: &str) -> Result<LakeAdminClient, String> {
        let (base, role) = match lake {
            Lake::Replica => (
                format!("https://{node}:{}", self.replica_port),
                REPLICA_ROLE,
            ),
            Lake::Pool(_) => match &self.main {
                Some(url) => (url.clone(), crate::lake_admin_client::ADAPTER_ROLE),
                None => return Err("no LAKE_ADMIN_URL: the main lake's adapter".into()),
            },
        };
        let (ca, identity) = self.pems.read()?;
        let client = LakeAdminClient::new(&base, &ca, &identity)?;
        Ok(client
            .accepting(role)
            .with_keys(self.keys.clone(), lake.clone()))
    }
}

impl LakeAdmins for AdapterLakeAdmins {
    fn for_lake(&self, lake: &Lake, node: &str) -> Arc<dyn LakeAdmin> {
        match self.client(lake, node) {
            Ok(client) => Arc::new(client),
            Err(e) => Arc::new(NotReached(e)),
        }
    }
}

/// A lake whose adapter cannot be asked from here: every call says why.
struct NotReached(String);

impl NotReached {
    fn answer<T>(&self) -> Result<T, LakeError> {
        Err(LakeError::Unavailable(self.0.clone()))
    }
}

#[async_trait]
impl LakeAdmin for NotReached {
    async fn ensure_warehouse(
        &self,
        _: &str,
        _: &RetentionRules,
    ) -> Result<WarehouseMade, LakeError> {
        self.answer()
    }

    async fn replicate(&self, _: &str, _: &ReplicaTarget) -> Result<ReplicationState, LakeError> {
        self.answer()
    }

    async fn replication(&self, _: &str) -> Result<ReplicationState, LakeError> {
        self.answer()
    }

    async fn set_managed_policies(&self, _: &[ManagedPolicy]) -> Result<(), LakeError> {
        self.answer()
    }

    async fn session_policy(&self, _: &LakeGrant) -> Result<SessionPolicy, LakeError> {
        self.answer()
    }

    async fn ensure_credentials(
        &self,
        _: LakeCredentialKind,
        _: Option<&LakeKeys>,
    ) -> Result<LakeKeys, LakeError> {
        self.answer()
    }

    async fn health(&self) -> Result<LakeHealth, LakeError> {
        self.answer()
    }
}
