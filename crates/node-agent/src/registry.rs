//! A cluster's machine pulls its role's images with the registry token the
//! core hands out (quality-sdk decision 0013): it asks `GET
//! /internal/machines/registry` with its certificate ([`RegistryGate`]); on
//! an answer it logs in with a fresh configuration directory of its own
//! (0700, inside the agent's directory), pulls each image by digest, and
//! removes the directory whatever happens ([`pull_role`]). The password is
//! never logged nor written anywhere but that directory, for as long as
//! the pull lasts.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::api::CallError;
use crate::cert::Credentials;
use crate::restart::identity;
use crate::store::FileStore;

/// How often a machine asks when it got an answer, or its role has none.
pub const ASK_EVERY: Duration = Duration::from_secs(6 * 3600);
/// How soon it asks again when the core holds no token, or did not answer.
pub const RETRY_AFTER: Duration = Duration::from_secs(10 * 60);

/// The core's answer: the token and the role's images.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct RegistryGrant {
    pub host: String,
    pub username: String,
    pub password: String,
    pub expires_at: String,
    pub images: Vec<String>,
}

impl std::fmt::Debug for RegistryGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryGrant")
            .field("host", &self.host)
            .field("expires_at", &self.expires_at)
            .field("images", &self.images)
            .finish_non_exhaustive()
    }
}

/// `GET /internal/machines/registry`, presenting the machine's certificate.
#[async_trait]
pub trait RegistryGate: Send + Sync {
    async fn registry(&self, identity: &Credentials, ca: &str) -> Result<RegistryGrant, CallError>;
}

/// The Docker CLI with a configuration directory of the caller's.
#[async_trait]
pub trait Docker: Send + Sync {
    /// `docker --config <config> login --username <user> --password-stdin
    /// <host>`, the password on its input.
    async fn login(&self, config: &Path, grant: &RegistryGrant) -> Result<(), String>;
    /// `docker --config <config> pull <image>`.
    async fn pull(&self, config: &Path, image: &str) -> Result<(), String>;
}

/// How a round ended, and so when to ask again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pulled {
    /// Every image pulled.
    All(usize),
    /// No installed module declares the machine's role, or it is not
    /// approved (403): nothing to pull.
    NotForRole,
    /// The core holds no token (503), did not answer, or a pull failed.
    Retry(String),
}

impl Pulled {
    /// When to ask again.
    pub fn next_in(&self) -> Duration {
        match self {
            Self::All(_) | Self::NotForRole => ASK_EVERY,
            Self::Retry(_) => RETRY_AFTER,
        }
    }
}

/// Ask the core and, with an answer, pull the role's images.
pub async fn pull_role(store: &FileStore, gate: &dyn RegistryGate, docker: &dyn Docker) -> Pulled {
    let (identity, ca) = match identity(store) {
        Ok(found) => found,
        Err(e) => return Pulled::Retry(format!("no certificate yet: {e}")),
    };
    let grant = match gate.registry(&identity, &ca).await {
        Ok(grant) => grant,
        Err(CallError::Rejected(403) | CallError::Refused) => return Pulled::NotForRole,
        Err(e) => return Pulled::Retry(format!("registry: {e}")),
    };
    let config = store.path(&format!("docker-config-{}", std::process::id()));
    let pulled = pull_with(&config, &grant, docker).await;
    if let Err(e) = remove(&config) {
        tracing::error!("the registry's configuration directory could not be removed: {e}");
    }
    match pulled {
        Ok(()) => Pulled::All(grant.images.len()),
        Err(e) => Pulled::Retry(e),
    }
}

async fn pull_with(
    config: &Path,
    grant: &RegistryGrant,
    docker: &dyn Docker,
) -> Result<(), String> {
    private_dir(config).map_err(|e| format!("configuration directory: {e}"))?;
    docker
        .login(config, grant)
        .await
        .map_err(|e| format!("login to {}: {e}", grant.host))?;
    for image in &grant.images {
        docker
            .pull(config, image)
            .await
            .map_err(|e| format!("pull {image}: {e}"))?;
    }
    Ok(())
}

/// A new, empty directory only its owner reads (0700).
fn private_dir(dir: &Path) -> std::io::Result<()> {
    remove(dir)?;
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

fn remove(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The Docker CLI as processes; `program` is `docker`.
#[derive(Debug, Clone)]
pub struct DockerCli {
    pub program: PathBuf,
    pub timeout: Duration,
}

impl Default for DockerCli {
    fn default() -> Self {
        Self {
            program: PathBuf::from("docker"),
            timeout: Duration::from_secs(15 * 60),
        }
    }
}

#[async_trait]
impl Docker for DockerCli {
    async fn login(&self, config: &Path, grant: &RegistryGrant) -> Result<(), String> {
        let args = [
            "login",
            "--username",
            grant.username.as_str(),
            "--password-stdin",
            grant.host.as_str(),
        ];
        self.run(config, &args, Some(grant.password.clone())).await
    }

    async fn pull(&self, config: &Path, image: &str) -> Result<(), String> {
        self.run(config, &["pull", "--quiet", image], None).await
    }
}

impl DockerCli {
    /// Runs `docker --config <config> <args>` with `input` on its standard
    /// input; its output is not kept (a login's could echo the user).
    async fn run(&self, config: &Path, args: &[&str], input: Option<String>) -> Result<(), String> {
        use std::process::{Command, Stdio};
        let mut command = Command::new(&self.program);
        command
            .arg("--config")
            .arg(config)
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let what = args.first().copied().unwrap_or_default().to_string();
        let run = tokio::task::spawn_blocking(move || -> Result<bool, String> {
            let mut child = command.spawn().map_err(|e| e.to_string())?;
            if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
                std::io::Write::write_all(&mut stdin, input.as_bytes())
                    .map_err(|e| e.to_string())?;
            }
            Ok(child.wait().map_err(|e| e.to_string())?.success())
        });
        match tokio::time::timeout(self.timeout, run).await {
            Ok(Ok(Ok(true))) => Ok(()),
            Ok(Ok(Ok(false))) => Err(format!("docker {what} failed")),
            Ok(Ok(Err(e))) => Err(format!("docker {what}: {e}")),
            Ok(Err(e)) => Err(format!("docker {what}: {e}")),
            Err(_) => Err(format!(
                "docker {what}: no exit in {}s",
                self.timeout.as_secs()
            )),
        }
    }
}

#[cfg(test)]
mod tests;
