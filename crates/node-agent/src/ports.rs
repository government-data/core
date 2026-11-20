//! The agent's other ports: the time, the commands it runs (the hook
//! after a renewal, and in held mode the question whether the service may
//! take its next certificate), and who is told how each renewal ended (the
//! binary's metrics file, decision 0071). Each with the real one and one
//! for tests.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;

/// The time, and waiting.
#[async_trait]
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
    async fn sleep(&self, wait: Duration);
}

/// The system's clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

#[async_trait]
impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    async fn sleep(&self, wait: Duration) {
        tokio::time::sleep(wait).await;
    }
}

/// A clock that moves only when the agent sleeps or a test advances it;
/// clones share it.
#[derive(Debug, Clone)]
pub struct ManualClock(Arc<Mutex<SystemTime>>);

impl ManualClock {
    pub fn at(now: SystemTime) -> Self {
        Self(Arc::new(Mutex::new(now)))
    }

    pub fn advance(&self, by: Duration) {
        let mut now = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        *now += by;
    }
}

#[async_trait]
impl Clock for ManualClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn sleep(&self, wait: Duration) {
        self.advance(wait);
    }
}

/// How a renewal ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Renewal {
    /// A new certificate, kept as the next one.
    Ok,
    /// Not this time: no answer, an error, an answer not taken.
    Failed,
    /// The API refused the machine: retired, or its certificate replaced.
    Refused,
}

impl Renewal {
    pub const ALL: [Self; 3] = [Self::Ok, Self::Failed, Self::Refused];

    /// Its name as a metric's label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Refused => "refused",
        }
    }
}

/// Told how each renewal ended.
pub trait Renewals: Send + Sync {
    fn renewed(&self, how: Renewal);
}

/// Nobody: the agent's default.
impl Renewals for () {
    fn renewed(&self, _: Renewal) {}
}

/// Every renewal's end, in order; clones share them.
#[derive(Debug, Clone, Default)]
pub struct RecordedRenewals(Arc<Mutex<Vec<Renewal>>>);

impl RecordedRenewals {
    pub fn all(&self) -> Vec<Renewal> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Renewals for RecordedRenewals {
    fn renewed(&self, how: Renewal) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(how);
    }
}

/// Runs a command, without a shell.
#[async_trait]
pub trait Commands: Send + Sync {
    /// Whether it exited 0; an error when it could not run or took too long.
    async fn run(&self, argv: &[String]) -> Result<bool, String>;
}

/// Commands as processes, killed after [`Self::timeout`].
#[derive(Debug, Clone, Copy)]
pub struct Processes {
    pub timeout: Duration,
}

impl Default for Processes {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(300),
        }
    }
}

#[async_trait]
impl Commands for Processes {
    async fn run(&self, argv: &[String]) -> Result<bool, String> {
        let (program, args) = argv.split_first().ok_or("an empty command")?;
        let mut command = tokio::process::Command::new(program);
        command.args(args).kill_on_drop(true);
        match tokio::time::timeout(self.timeout, command.status()).await {
            Ok(Ok(status)) => Ok(status.success()),
            Ok(Err(e)) => Err(format!("{program}: {e}")),
            Err(_) => Err(format!("{program}: no exit in {}s", self.timeout.as_secs())),
        }
    }
}

/// Commands that run nothing: what each was asked, and the answers given
/// in turn (`true` when they run out).
#[derive(Debug, Clone, Default)]
pub struct ScriptedCommands {
    ran: Arc<Mutex<Vec<Vec<String>>>>,
    answers: Arc<Mutex<VecDeque<bool>>>,
}

impl ScriptedCommands {
    /// The next commands answer these, in order.
    pub fn answering(&self, answers: &[bool]) {
        let mut queue = self.answers.lock().unwrap_or_else(PoisonError::into_inner);
        queue.extend(answers);
    }

    /// Every command run so far.
    pub fn ran(&self) -> Vec<Vec<String>> {
        self.ran
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl Commands for ScriptedCommands {
    async fn run(&self, argv: &[String]) -> Result<bool, String> {
        let mut ran = self.ran.lock().unwrap_or_else(PoisonError::into_inner);
        ran.push(argv.to_vec());
        let mut answers = self.answers.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(answers.pop_front().unwrap_or(true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn processes_say_whether_they_exited_zero() {
        let run = |argv: &[&str]| {
            let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
            async move { Processes::default().run(&argv).await }
        };
        assert_eq!(run(&["true"]).await, Ok(true));
        assert_eq!(run(&["false"]).await, Ok(false));
        assert!(run(&["/nonexistent/qd-hook"]).await.is_err());
        assert!(run(&[]).await.is_err());
        let slow = Processes {
            timeout: Duration::from_millis(50),
        };
        let sleep = ["sleep".to_string(), "5".to_string()];
        assert!(slow.run(&sleep).await.unwrap_err().contains("no exit"));
    }

    #[tokio::test]
    async fn the_manual_clock_moves_when_slept_on() {
        let clock = ManualClock::at(SystemTime::UNIX_EPOCH);
        clock.sleep(Duration::from_secs(5)).await;
        clock.clone().advance(Duration::from_secs(1));
        assert_eq!(clock.now(), SystemTime::UNIX_EPOCH + Duration::from_secs(6));
    }
}
