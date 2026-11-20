//! The files a new machine is installed with (decision 0029, 0066 step 11),
//! embedded at build: `infra/join/join.sh`, each role's compose file, the
//! entrypoints it names and its Alloy configuration (decision 0071), with
//! the SHA-256 of their bytes, and the
//! installation's CA with its fingerprint (the SHA-256 of its DER, as
//! `server ca-init` prints it and the node agent checks it).
//!
//! `join.sh` names the SHA-256 of each file it downloads, so the command's
//! one hash of the script covers them too; a test keeps them in step.

use auth_gateway::{InstallFile, InstallFiles, MachineRole};
use sha2::{Digest, Sha256};

use crate::ca::{sha256_fingerprint, CaError};

/// `infra/join/join.sh`.
pub const JOIN_SCRIPT: &str = include_str!("../../../../infra/join/join.sh");

/// `infra/worker/docker-compose.yml`, the worker's.
pub const WORKER_COMPOSE: &str = include_str!("../../../../infra/worker/docker-compose.yml");

/// `infra/rustfs/replica/docker-compose.yml`, the lake replica's.
pub const LAKE_REPLICA_COMPOSE: &str =
    include_str!("../../../../infra/rustfs/replica/docker-compose.yml");

/// `infra/rustfs/pool/docker-compose.yml`, a lake pool node's.
pub const LAKE_POOL_COMPOSE: &str =
    include_str!("../../../../infra/rustfs/pool/docker-compose.yml");

/// `infra/worker/lake/entrypoint.sh`, served to a pool node as
/// `lake-proxy.sh`, its replication's proxy.
pub const LAKE_PROXY_ENTRYPOINT: &str = include_str!("../../../../infra/worker/lake/entrypoint.sh");

/// `infra/rustfs/tls/entrypoint.sh`, served as `rustfs-tls.sh`.
pub const RUSTFS_TLS_ENTRYPOINT: &str = include_str!("../../../../infra/rustfs/tls/entrypoint.sh");

/// `infra/rustfs/gate/entrypoint.sh`, served as `lake-gate.sh`.
pub const LAKE_GATE_ENTRYPOINT: &str = include_str!("../../../../infra/rustfs/gate/entrypoint.sh");

/// `infra/observability/alloy/worker.alloy`, a worker's Alloy, served as
/// `worker.alloy`.
pub const WORKER_ALLOY: &str = include_str!("../../../../infra/observability/alloy/worker.alloy");

/// `infra/observability/alloy/lake.alloy`, a lake pool node's Alloy, served
/// as `lake.alloy`.
pub const LAKE_ALLOY: &str = include_str!("../../../../infra/observability/alloy/lake.alloy");

/// `infra/observability/alloy/lake-replica.alloy`, the lake replica's
/// Alloy, served as `lake-replica.alloy`.
pub const LAKE_REPLICA_ALLOY: &str =
    include_str!("../../../../infra/observability/alloy/lake-replica.alloy");

/// The SHA-256 of `bytes`, lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// [`InstallFiles`] from what was embedded at build, and the CA given at
/// start.
#[derive(Debug, Clone)]
pub struct EmbeddedInstallFiles {
    script: InstallFile,
    worker: InstallFile,
    replica: InstallFile,
    pool: InstallFile,
    lake_proxy: InstallFile,
    rustfs_tls: InstallFile,
    lake_gate: InstallFile,
    worker_alloy: InstallFile,
    replica_alloy: InstallFile,
    lake_alloy: InstallFile,
    ca: Option<InstallFile>,
}

impl EmbeddedInstallFiles {
    /// With the installation's CA certificate (PEM), if it has one.
    pub fn new(ca_pem: Option<&str>) -> Result<Self, CaError> {
        let ca = match ca_pem {
            Some(pem) => Some(InstallFile {
                body: pem.to_string(),
                sha256: sha256_fingerprint(pem)?,
            }),
            None => None,
        };
        Ok(Self {
            script: embedded(JOIN_SCRIPT),
            worker: embedded(WORKER_COMPOSE),
            replica: embedded(LAKE_REPLICA_COMPOSE),
            pool: embedded(LAKE_POOL_COMPOSE),
            lake_proxy: embedded(LAKE_PROXY_ENTRYPOINT),
            rustfs_tls: embedded(RUSTFS_TLS_ENTRYPOINT),
            lake_gate: embedded(LAKE_GATE_ENTRYPOINT),
            worker_alloy: embedded(WORKER_ALLOY),
            replica_alloy: embedded(LAKE_REPLICA_ALLOY),
            lake_alloy: embedded(LAKE_ALLOY),
            ca,
        })
    }
}

fn embedded(body: &str) -> InstallFile {
    InstallFile {
        body: body.to_string(),
        sha256: sha256_hex(body.as_bytes()),
    }
}

impl InstallFiles for EmbeddedInstallFiles {
    fn script(&self) -> InstallFile {
        self.script.clone()
    }

    fn compose(&self, role: MachineRole) -> Option<InstallFile> {
        match role {
            MachineRole::Worker => Some(self.worker.clone()),
            MachineRole::LakeReplica => Some(self.replica.clone()),
            MachineRole::Lake => Some(self.pool.clone()),
            _ => None,
        }
    }

    fn file(&self, role: MachineRole, name: &str) -> Option<InstallFile> {
        let lake = matches!(role, MachineRole::LakeReplica | MachineRole::Lake);
        match name {
            "rustfs-tls.sh" if lake => Some(self.rustfs_tls.clone()),
            "lake-gate.sh" if lake => Some(self.lake_gate.clone()),
            "lake-proxy.sh" if role == MachineRole::Lake => Some(self.lake_proxy.clone()),
            "worker.alloy" if role == MachineRole::Worker => Some(self.worker_alloy.clone()),
            "lake.alloy" if role == MachineRole::Lake => Some(self.lake_alloy.clone()),
            "lake-replica.alloy" if role == MachineRole::LakeReplica => {
                Some(self.replica_alloy.clone())
            }
            _ => None,
        }
    }

    fn ca(&self) -> Option<InstallFile> {
        self.ca.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ca::{Ca, DEFAULT_CA_VALIDITY};

    #[test]
    fn the_hashes_are_those_of_the_embedded_bytes() {
        let files = EmbeddedInstallFiles::new(None).unwrap();
        let script = files.script();
        assert_eq!(script.body, JOIN_SCRIPT);
        assert_eq!(script.sha256, sha256_hex(JOIN_SCRIPT.as_bytes()));
        assert_eq!(script.sha256.len(), 64);
        let worker = files.compose(MachineRole::Worker).unwrap();
        assert_eq!(worker.body, WORKER_COMPOSE);
        assert_eq!(worker.sha256, sha256_hex(WORKER_COMPOSE.as_bytes()));
        let replica = files.compose(MachineRole::LakeReplica).unwrap();
        assert_eq!(replica.sha256, sha256_hex(LAKE_REPLICA_COMPOSE.as_bytes()));
        let tls = files.file(MachineRole::LakeReplica, "rustfs-tls.sh");
        assert_eq!(tls.unwrap().body, RUSTFS_TLS_ENTRYPOINT);
        let gate = files.file(MachineRole::LakeReplica, "lake-gate.sh");
        assert_eq!(gate.unwrap().body, LAKE_GATE_ENTRYPOINT);
        assert_eq!(files.file(MachineRole::Worker, "rustfs-tls.sh"), None);
        assert_eq!(files.file(MachineRole::LakeReplica, "compose.yml"), None);
        assert_eq!(files.file(MachineRole::LakeReplica, "../join.sh"), None);
        let pool = files.compose(MachineRole::Lake).unwrap();
        assert_eq!(pool.sha256, sha256_hex(LAKE_POOL_COMPOSE.as_bytes()));
        let proxy = files.file(MachineRole::Lake, "lake-proxy.sh").unwrap();
        assert_eq!(proxy.body, LAKE_PROXY_ENTRYPOINT);
        assert!(files.file(MachineRole::Lake, "rustfs-tls.sh").is_some());
        assert_eq!(files.file(MachineRole::LakeReplica, "lake-proxy.sh"), None);
        let alloy = files.file(MachineRole::Worker, "worker.alloy").unwrap();
        assert_eq!(alloy.sha256, sha256_hex(WORKER_ALLOY.as_bytes()));
        assert_eq!(files.file(MachineRole::Lake, "worker.alloy"), None);
        let alloy = files.file(MachineRole::LakeReplica, "lake-replica.alloy");
        assert_eq!(alloy.unwrap().body, LAKE_REPLICA_ALLOY);
        assert_eq!(files.file(MachineRole::Lake, "lake-replica.alloy"), None);
        let alloy = files.file(MachineRole::Lake, "lake.alloy").unwrap();
        assert_eq!(alloy.sha256, sha256_hex(LAKE_ALLOY.as_bytes()));
        assert_eq!(files.file(MachineRole::LakeReplica, "lake.alloy"), None);
        assert_eq!(files.compose(MachineRole::TrinoWorker), None);
        assert_eq!(files.compose(MachineRole::App), None);
        assert_eq!(files.ca(), None);
        // A known vector: the SHA-256 of "abc".
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn the_ca_is_given_with_the_fingerprint_the_agent_checks() {
        let made = Ca::create("test", DEFAULT_CA_VALIDITY).unwrap();
        let files = EmbeddedInstallFiles::new(Some(&made.cert_pem)).unwrap();
        let ca = files.ca().unwrap();
        assert_eq!(ca.body, made.cert_pem);
        assert_eq!(ca.sha256, sha256_fingerprint(&made.cert_pem).unwrap());
        assert!(EmbeddedInstallFiles::new(Some("not a certificate")).is_err());
    }

    /// `join.sh` checks each file it downloads against the hash it names: a
    /// change to one is a change to the other.
    #[test]
    fn the_script_names_the_hash_of_each_file_it_downloads() {
        for (line, file, path) in [
            (
                "WORKER_COMPOSE_SHA256=",
                WORKER_COMPOSE,
                "infra/worker/docker-compose.yml",
            ),
            (
                "LAKE_REPLICA_COMPOSE_SHA256=",
                LAKE_REPLICA_COMPOSE,
                "infra/rustfs/replica/docker-compose.yml",
            ),
            (
                "LAKE_POOL_COMPOSE_SHA256=",
                LAKE_POOL_COMPOSE,
                "infra/rustfs/pool/docker-compose.yml",
            ),
            (
                "LAKE_PROXY_SHA256=",
                LAKE_PROXY_ENTRYPOINT,
                "infra/worker/lake/entrypoint.sh",
            ),
            (
                "RUSTFS_TLS_SHA256=",
                RUSTFS_TLS_ENTRYPOINT,
                "infra/rustfs/tls/entrypoint.sh",
            ),
            (
                "LAKE_GATE_SHA256=",
                LAKE_GATE_ENTRYPOINT,
                "infra/rustfs/gate/entrypoint.sh",
            ),
            (
                "LAKE_ALLOY_SHA256=",
                LAKE_ALLOY,
                "infra/observability/alloy/lake.alloy",
            ),
            (
                "LAKE_REPLICA_ALLOY_SHA256=",
                LAKE_REPLICA_ALLOY,
                "infra/observability/alloy/lake-replica.alloy",
            ),
            (
                "WORKER_ALLOY_SHA256=",
                WORKER_ALLOY,
                "infra/observability/alloy/worker.alloy",
            ),
        ] {
            let named: Vec<&str> = JOIN_SCRIPT
                .lines()
                .filter_map(|l| l.strip_prefix(line))
                .collect();
            assert_eq!(
                named,
                [sha256_hex(file.as_bytes()).as_str()],
                "set {line} in infra/join/join.sh to the SHA-256 of {path}"
            );
        }
    }

    /// The main lake's admin user is made by `infra/rustfs/admin-user.sh`
    /// as `join.sh` makes the replica's (0066 step 11, ruling 1): the same
    /// user and the same policy.
    #[test]
    fn the_main_lake_gets_the_admin_user_join_sh_makes() {
        const MAIN: &str = include_str!("../../../../infra/rustfs/admin-user.sh");
        for name in ["ADMIN_USER=", "ADMIN_POLICY="] {
            let line = |script: &'static str| -> Vec<&'static str> {
                script.lines().filter(|l| l.starts_with(name)).collect()
            };
            assert_eq!(line(JOIN_SCRIPT).len(), 1, "{name} in join.sh");
            assert_eq!(
                line(MAIN),
                line(JOIN_SCRIPT),
                "keep {name} in infra/rustfs/admin-user.sh as infra/join/join.sh has it"
            );
        }
    }
}
