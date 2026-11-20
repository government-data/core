//! The agent's loop: each [`Agent::step`] asks [`crate::plan::decide`]
//! what to do with the files and the time it has, and does it. A step
//! either succeeds, fails in a way worth trying again (it waits, backing
//! off), or fails for good ([`Fatal`]: the binary exits non-zero).

use std::io;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::api::{CallError, MachineApi};
use crate::cert::Validity;
use crate::config::Settings;
use crate::plan::{backoff, decide, first_renewal, jittered, Facts, Step};
use crate::ports::{Clock, Commands, Renewals};
use crate::store::FileStore;

mod steps;

/// Why the agent stops.
#[derive(Debug)]
pub enum Fatal {
    /// A setting is missing or wrong.
    Settings(String),
    /// The join token was refused: unknown, used or expired.
    JoinRefused,
    /// Another machine has the name.
    NameTaken,
    /// The API refused the certificate: the machine was retired, or its
    /// certificate replaced. Remembered, so a restart does not ask again.
    Refused,
    /// The join answered with a CA other than `QD_CA_SHA256`'s.
    CaMismatch,
    /// A renewal answered with a CA other than `ca.crt`.
    CaChanged,
    /// The certificate ended and there is no other.
    Ended,
    /// The API rejected the call in a way asking again would not mend.
    Rejected(CallError),
    /// The certificate answered is not for the key sent, or unreadable.
    Answer(String),
    Store(io::Error),
}

impl std::fmt::Display for Fatal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Settings(e) => write!(f, "settings: {e}"),
            Self::JoinRefused => f.write_str(
                "the join token was refused (unknown, used or expired): issue another in \
                 Settings → Execution, and retire the machine left pending, if any",
            ),
            Self::NameTaken => f.write_str(
                "another machine has this name: choose another QD_MACHINE_NAME, or retire it",
            ),
            Self::Refused => f.write_str(
                "the API refused this machine's certificate: it was retired or its certificate \
                 replaced; it must join again with a new token (empty QD_AGENT_DIR first)",
            ),
            Self::CaMismatch => f.write_str(
                "the CA the join answered is not the one of QD_CA_SHA256: nothing was written",
            ),
            Self::CaChanged => f.write_str(
                "the CA a renewal answered is not ca.crt: changing the CA is not supported yet",
            ),
            Self::Ended => f.write_str(
                "the certificate ended before it could be renewed: the machine must join again",
            ),
            Self::Rejected(e) => write!(f, "the API rejected the call: {e}"),
            Self::Answer(e) => write!(f, "the API's answer: {e}"),
            Self::Store(e) => write!(f, "the agent's files: {e}"),
        }
    }
}

impl std::error::Error for Fatal {}

impl From<io::Error> for Fatal {
    fn from(e: io::Error) -> Self {
        Self::Store(e)
    }
}

/// The agent, over its ports.
pub struct Agent<A, C, R> {
    settings: Settings,
    store: FileStore,
    api: A,
    clock: C,
    commands: R,
    /// Told how each renewal ended (the metrics file).
    renewals: Arc<dyn Renewals>,
    /// A number in [-1, 1] for the jitter.
    jitter: fn() -> f64,
    /// When to call the API next.
    call_at: SystemTime,
    /// When to ask, in held mode, whether the next may be taken.
    ask_at: SystemTime,
    /// Calls failed in a row.
    failures: u32,
}

impl<A, C, R> std::fmt::Debug for Agent<A, C, R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("settings", &self.settings)
            .field("call_at", &self.call_at)
            .field("failures", &self.failures)
            .finish_non_exhaustive()
    }
}

impl<A: MachineApi, C: Clock, R: Commands> Agent<A, C, R> {
    pub fn new(settings: Settings, store: FileStore, api: A, clock: C, commands: R) -> Self {
        let now = clock.now();
        Self {
            settings,
            store,
            api,
            clock,
            commands,
            renewals: Arc::new(()),
            jitter: random_unit,
            call_at: now,
            ask_at: now,
            failures: 0,
        }
    }

    /// The same agent with another source of jitter (tests: none).
    pub fn with_jitter(mut self, jitter: fn() -> f64) -> Self {
        self.jitter = jitter;
        self
    }

    /// The same agent, telling `renewals` how each renewal ends.
    pub fn with_renewals(mut self, renewals: Arc<dyn Renewals>) -> Self {
        self.renewals = renewals;
        self
    }

    pub fn store(&self) -> &FileStore {
        &self.store
    }

    /// Reads what it holds and schedules the first call: a machine refused
    /// before stops here, without asking the API again.
    pub fn start(&mut self) -> Result<(), Fatal> {
        if self.store.refused() {
            return Err(Fatal::Refused);
        }
        let now = self.clock.now();
        self.call_at = now;
        self.ask_at = now;
        if let Some(current) = self.current()? {
            self.call_at = first_renewal(now, &current, self.period());
            let left = current.remaining(now);
            tracing::info!(left = %human(left), "certificate in use");
        }
        Ok(())
    }

    /// Runs until something fatal.
    pub async fn run(mut self) -> Fatal {
        if let Err(e) = self.start() {
            return e;
        }
        loop {
            if let Err(e) = self.step().await {
                return e;
            }
        }
    }

    /// Decides and does one thing; says what it was.
    pub async fn step(&mut self) -> Result<Step, Fatal> {
        let facts = self.facts()?;
        let step = decide(self.settings.mode, &facts);
        match step {
            Step::Join => self.join().await?,
            Step::Promote => self.promote().await?,
            Step::Renew => self.renew(facts.current).await?,
            Step::AskReady => self.ask_ready().await?,
            Step::Ended => return Err(Fatal::Ended),
            Step::Wait(wait) => self.clock.sleep(wait).await,
        }
        Ok(step)
    }

    fn facts(&self) -> Result<Facts, Fatal> {
        let next = match self.store.next()? {
            Some(next) => Some(next.validity().map_err(|e| Fatal::Answer(e.clone()))?),
            None => None,
        };
        Ok(Facts {
            now: self.clock.now(),
            current: self.current()?,
            next,
            call_at: self.call_at,
            ask_at: self.ask_at,
        })
    }

    fn current(&self) -> Result<Option<Validity>, Fatal> {
        let Some(current) = self.store.current()? else {
            return Ok(None);
        };
        let invalid = |e: String| Fatal::Store(io::Error::new(io::ErrorKind::InvalidData, e));
        current.validity().map(Some).map_err(invalid)
    }

    fn period(&self) -> Duration {
        jittered(self.settings.renew_every, (self.jitter)())
    }

    /// A call failed in a way worth trying again: when, and what it means.
    fn failed(&mut self, what: &str, e: &CallError, current: Option<Validity>) {
        self.failures = self.failures.saturating_add(1);
        let mut wait = backoff(self.failures);
        if let CallError::TooMany(after) = e {
            wait = wait.max(*after);
        }
        let now = self.clock.now();
        self.call_at = now + wait;
        let left = current.map_or_else(|| "-".to_string(), |c| human(c.remaining(now)));
        tracing::warn!(attempt = self.failures, retry_in = %human(wait), certificate_left = %left, "{what}: {e}");
    }

    fn succeeded(&mut self) {
        self.failures = 0;
        self.call_at = self.clock.now() + self.period();
    }

    /// The hook after a new certificate is in place; a failure is only
    /// logged: the files are there, and the next renewal runs it again.
    async fn after_renew(&self) {
        let Some(hook) = &self.settings.after_renew else {
            return;
        };
        match self.commands.run(hook).await {
            Ok(true) => tracing::info!("QD_AFTER_RENEW ran"),
            Ok(false) => tracing::warn!("QD_AFTER_RENEW exited non-zero"),
            Err(e) => tracing::warn!("QD_AFTER_RENEW: {e}"),
        }
    }
}

/// `5h07m`, `42s`: how long, for an operator.
pub fn human(d: Duration) -> String {
    let s = d.as_secs();
    match (s / 3600, (s % 3600) / 60) {
        (0, 0) => format!("{s}s"),
        (0, m) => format!("{m}m"),
        (h, m) => format!("{h}h{m:02}m"),
    }
}

/// A number in [-1, 1] from the system's randomness; 0 if it fails.
fn random_unit() -> f64 {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 4];
    if ring::rand::SystemRandom::new().fill(&mut bytes).is_err() {
        return 0.0;
    }
    f64::from(u32::from_le_bytes(bytes)) / f64::from(u32::MAX) * 2.0 - 1.0
}

#[cfg(test)]
mod tests;
