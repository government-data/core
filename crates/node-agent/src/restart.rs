//! A service that reads its certificate only when it starts is restarted
//! to take a new one only when it is free, through the API's hold (0066,
//! points 9 and 11): Trino's coordinator ([`crate::trino`]) and Dolt
//! ([`crate::dolt`]). What they share is here: the restart command, the
//! wait until the service answers again (each says how in its [`Probe`]),
//! and the release with the certificate now in use, tried again on what
//! may pass.
//!
//! If the service does not come back within the wait the hold is not
//! released: work let go against a service that does not answer would
//! fail. The API ends the hold by itself (15 minutes after it was taken for
//! Trino, 5 for Dolt), which is longer than the wait, and the failure is
//! logged for a person.

use std::time::Duration;

use async_trait::async_trait;

use crate::api::CallError;
use crate::cert::Credentials;
use crate::ports::{Clock, Commands};
use crate::store::FileStore;

/// The API's release of a service's hold.
#[async_trait]
pub trait Release: Send + Sync {
    /// Whether a hold was in force.
    async fn release(&self, identity: &Credentials, ca: &str) -> Result<bool, CallError>;
}

/// Whether the service answers again since its restart.
#[async_trait]
pub trait Probe: Send + Sync {
    /// `since`: how long ago the restart command was run.
    async fn back(&self, since: Duration) -> bool;
}

/// How [`restart`] restarts the service and waits for it.
#[derive(Debug, Clone)]
pub struct Restart {
    /// The command, without a shell (`docker restart trino-coordinator`).
    pub command: Vec<String>,
    /// How long the service may take to answer again: within the 5 minutes
    /// a hook may run, and the API's hold.
    pub wait: Duration,
    /// How often it is asked meanwhile.
    pub poll: Duration,
}

/// Why the restart did not end with the hold released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartError {
    /// The agent's files cannot be read.
    Files(String),
    /// The command could not run, or exited non-zero.
    Command(String),
    /// The service did not answer within the wait.
    NotBack(Duration),
    /// The service is back but the hold could not be ended: it ends by itself.
    Release(CallError),
}

impl std::fmt::Display for RestartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Files(e) => write!(f, "the agent's files: {e}"),
            Self::Command(e) => write!(
                f,
                "the restart command: {e}; what uses it (jobs, operations) stay held until the hold expires"
            ),
            Self::NotBack(wait) => write!(
                f,
                "the service did not answer within {}s of its restart; what uses it (jobs, operations) stay held until \
                 the hold expires: look at the service",
                wait.as_secs()
            ),
            Self::Release(e) => write!(
                f,
                "the service is back, but the hold could not be ended ({e}): it expires by itself"
            ),
        }
    }
}

impl std::error::Error for RestartError {}

/// The certificate in use and the CA, from the agent's directory.
pub fn identity(store: &FileStore) -> Result<(Credentials, String), String> {
    match (store.current(), store.ca()) {
        (Ok(Some(identity)), Ok(Some(ca))) => Ok((identity, ca)),
        (Err(e), _) | (_, Err(e)) => Err(e.to_string()),
        _ => Err("machine.pem and ca.crt are needed".into()),
    }
}

/// Restarts the service, waits for `probe` to say it is back, and ends the
/// hold with the certificate now in use. Whether a hold was in force.
pub async fn restart(
    store: &FileStore,
    gate: &dyn Release,
    probe: &dyn Probe,
    commands: &dyn Commands,
    clock: &dyn Clock,
    how: &Restart,
) -> Result<bool, RestartError> {
    let began = clock.now();
    match commands.run(&how.command).await {
        Ok(true) => tracing::info!("the service restarted, waiting for it"),
        Ok(false) => return Err(RestartError::Command("exited non-zero".into())),
        Err(e) => return Err(RestartError::Command(e)),
    }
    loop {
        let since = clock.now().duration_since(began).unwrap_or_default();
        if probe.back(since + how.poll).await {
            tracing::info!(after_s = since.as_secs(), "the service answers again");
            break;
        }
        if since >= how.wait {
            return Err(RestartError::NotBack(how.wait));
        }
        clock.sleep(how.poll).await;
    }
    release(store, gate, clock).await
}

/// Ends the hold, trying again a few times on what may pass.
async fn release(
    store: &FileStore,
    gate: &dyn Release,
    clock: &dyn Clock,
) -> Result<bool, RestartError> {
    let (identity, ca) = identity(store).map_err(RestartError::Files)?;
    let mut tries = 0;
    loop {
        match gate.release(&identity, &ca).await {
            Ok(in_force) => return Ok(in_force),
            Err(e) if e.retryable() && tries < 4 => {
                tries += 1;
                tracing::warn!(attempt = tries, "releasing the hold: {e}");
                clock.sleep(Duration::from_secs(5 * tries)).await;
            }
            Err(e) => return Err(RestartError::Release(e)),
        }
    }
}
