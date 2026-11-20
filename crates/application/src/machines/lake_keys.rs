//! A lake's root keys, given to a node that is installing (decision 0029;
//! 0066 step 11, the user's ruling of 2026-11-13): RustFS needs them to
//! start, a pool's nodes the same, so the application makes them, once per
//! lake, and gives them, as an environment file, to whoever presents a join
//! token of a lake role that is unused and not expired, as it gives the
//! compose file; the token is not used up. Every time they are given it is
//! audited first, never with the keys. They are used only to install:
//! `join.sh` makes the platform's admin user with them once, on the node,
//! and nothing uses them after (decision 0028, point 2; 0066 step 11, the
//! user's ruling of 2026-11-13).

use std::sync::Arc;

use auth_gateway::{Lake, LakeRootKeyStore, MachineRole};

use super::{audit, InstallFileError, MachinesService, Origin};

impl MachinesService {
    /// With the store of the lakes' root keys.
    pub fn with_lake_keys(mut self, keys: Arc<dyn LakeRootKeyStore>) -> Self {
        self.lake_keys = Some(keys);
        self
    }

    /// RustFS's root keys of the lake a node of `role` belongs to, as the
    /// environment file RustFS reads (`RUSTFS_ACCESS_KEY`,
    /// `RUSTFS_SECRET_KEY`), for a usable join token of that role: the
    /// replica's, or for a pool's node the lake's, kept under `pool:main`
    /// and the same for every pool (0066 step 11, ruling 2 of 2026-11-14).
    pub async fn role_root_keys(
        &self,
        token: &str,
        role: MachineRole,
        origin: &Origin,
    ) -> Result<String, InstallFileError> {
        self.granted(token, role).await?;
        let lake = match role {
            MachineRole::LakeReplica => Lake::Replica,
            MachineRole::Lake => Lake::main(),
            _ => return Err(InstallFileError::NotFound),
        };
        let Some(store) = &self.lake_keys else {
            return Err(InstallFileError::Unavailable);
        };
        let backend = |e: domain::RepositoryError| InstallFileError::Backend(e.to_string());
        let keys = store.keys(&lake).await.map_err(backend)?;
        let event = audit::lake_keys_given(&lake, role, origin, self.clock.now());
        self.record(event).await.map_err(backend)?;
        Ok(format!(
            "RUSTFS_ACCESS_KEY={}\nRUSTFS_SECRET_KEY={}\n",
            keys.access_key, keys.secret_key
        ))
    }
}
