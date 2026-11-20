//! The worker guards itself (decision 0072, point 14): before each claim it
//! checks its disk and its memory, and while either is at its limit it
//! claims nothing. Handing out work never depends on Grafana or Mimir.
//!
//! - **Disk**: the free space (`statvfs`) of Docker's data, which is the
//!   filesystem under the container's root (`/`, Docker's overlay), against
//!   `QD_WORKER_MIN_FREE_BYTES` (default 5 GB). The worker has no spill path
//!   of its own (0072 step 1: DuckDB's temporary files are on the `/tmp`
//!   tmpfs, which is memory).
//! - **Memory**: the lower of the host's `MemAvailable` / `MemTotal` and the
//!   container's cgroup headroom (`memory.max` − `memory.current`, when it
//!   has a limit), against `QD_WORKER_MIN_FREE_MEMORY_PCT` (default 10 %).
//!
//! On a change it logs one line and sets `qd_worker_claim_paused{reason}`
//! (`disk`, `memory`), alerted as `worker_paused`. A reading that fails
//! does not stop the claim: the line says so once.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use quality::worker_metrics::WorkerMetrics;

/// The default minimum free disk, bytes: what `alerts.json`'s worker rules
/// take as the claim limit.
pub(crate) const DEFAULT_MIN_FREE_BYTES: u64 = 5_000_000_000;
/// The default minimum free memory, %.
pub(crate) const DEFAULT_MIN_FREE_MEMORY_PCT: f64 = 10.0;

/// What the guard reads.
pub(crate) trait Probe: Send + Sync {
    /// Free bytes for an unprivileged user on the filesystem of `path`.
    fn free_bytes(&self, path: &Path) -> Result<u64, String>;
    /// Free memory, %: the lower of the host's and the cgroup's.
    fn free_memory_pct(&self) -> Result<f64, String>;
}

/// Which limits are reached.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Paused {
    pub(crate) disk: bool,
    pub(crate) memory: bool,
}

impl Paused {
    fn any(self) -> bool {
        self.disk || self.memory
    }
}

/// The guard, shared by the slots.
pub(crate) struct Guard {
    paths: Vec<PathBuf>,
    min_free_bytes: u64,
    min_free_memory_pct: f64,
    probe: Box<dyn Probe>,
    metrics: WorkerMetrics,
    /// The last state, and whether a failed reading was logged.
    last: Mutex<(Paused, bool)>,
}

impl std::fmt::Debug for Guard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Guard")
            .field("paths", &self.paths)
            .field("min_free_bytes", &self.min_free_bytes)
            .field("min_free_memory_pct", &self.min_free_memory_pct)
            .finish_non_exhaustive()
    }
}

impl Guard {
    pub(crate) fn new(
        paths: Vec<PathBuf>,
        min_free_bytes: u64,
        min_free_memory_pct: f64,
        probe: Box<dyn Probe>,
        metrics: WorkerMetrics,
    ) -> Self {
        metrics.set_claim_paused(false, false);
        Self {
            paths,
            min_free_bytes,
            min_free_memory_pct,
            probe,
            metrics,
            last: Mutex::default(),
        }
    }

    /// From `QD_WORKER_MIN_FREE_BYTES` and `QD_WORKER_MIN_FREE_MEMORY_PCT`,
    /// over this host: Docker's data under `/`.
    pub(crate) fn from_env(metrics: WorkerMetrics) -> Result<Self, String> {
        let bytes = setting("QD_WORKER_MIN_FREE_BYTES", DEFAULT_MIN_FREE_BYTES)?;
        let pct = setting("QD_WORKER_MIN_FREE_MEMORY_PCT", DEFAULT_MIN_FREE_MEMORY_PCT)?;
        if !(0.0..100.0).contains(&pct) {
            return Err("QD_WORKER_MIN_FREE_MEMORY_PCT: from 0 to under 100".into());
        }
        let paths = vec![PathBuf::from("/")];
        Ok(Self::new(paths, bytes, pct, Box::new(Host), metrics))
    }

    /// Runs `claim` only while no limit is reached; `None` otherwise.
    pub(crate) async fn claim<F, Fut>(&self, claim: F) -> Option<Fut::Output>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future,
    {
        if self.check().any() {
            return None;
        }
        Some(claim().await)
    }

    /// Reads both limits; on a change, one log line and the gauge.
    pub(crate) fn check(&self) -> Paused {
        let mut failed = Vec::new();
        let mut disk = false;
        for path in &self.paths {
            match self.probe.free_bytes(path) {
                Ok(free) => disk |= free < self.min_free_bytes,
                Err(e) => failed.push(format!("{}: {e}", path.display())),
            }
        }
        let memory = match self.probe.free_memory_pct() {
            Ok(pct) => pct < self.min_free_memory_pct,
            Err(e) => {
                failed.push(format!("memory: {e}"));
                false
            }
        };
        let now = Paused { disk, memory };
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if !failed.is_empty() && !last.1 {
            tracing::warn!(failed = %failed.join("; "), "the claim guard could not read a limit");
        }
        last.1 = !failed.is_empty();
        if now != last.0 {
            if now.any() {
                tracing::warn!(disk, memory, "at its limit: no job is claimed");
            } else {
                tracing::info!("under its limits again: claiming jobs");
            }
            self.metrics.set_claim_paused(disk, memory);
            last.0 = now;
        }
        now
    }
}

fn setting<T: std::str::FromStr>(name: &str, default: T) -> Result<T, String> {
    match crate::config::env(name) {
        Some(raw) => raw
            .trim()
            .parse()
            .map_err(|_| format!("{name}: not a number")),
        None => Ok(default),
    }
}

/// This host, as the container sees it.
struct Host;

impl Probe for Host {
    fn free_bytes(&self, path: &Path) -> Result<u64, String> {
        let st = rustix::fs::statvfs(path).map_err(|e| e.to_string())?;
        Ok(st.f_bavail.saturating_mul(st.f_frsize))
    }

    fn free_memory_pct(&self) -> Result<f64, String> {
        let meminfo = std::fs::read_to_string("/proc/meminfo").map_err(|e| e.to_string())?;
        let host = host_free_pct(&meminfo).ok_or("no MemTotal or MemAvailable")?;
        let read = |f: &str| std::fs::read_to_string(Path::new("/sys/fs/cgroup").join(f)).ok();
        let cgroup = match (read("memory.max"), read("memory.current")) {
            (Some(max), Some(current)) => cgroup_free_pct(&max, &current),
            _ => None,
        };
        Ok(cgroup.map_or(host, |c| c.min(host)))
    }
}

/// `MemAvailable` / `MemTotal`, %, from `/proc/meminfo`.
pub(crate) fn host_free_pct(meminfo: &str) -> Option<f64> {
    let field = |name: &str| -> Option<f64> {
        let line = meminfo.lines().find(|l| l.starts_with(name))?;
        line[name.len()..].split_whitespace().next()?.parse().ok()
    };
    let (total, available) = (field("MemTotal:")?, field("MemAvailable:")?);
    (total > 0.0).then(|| available / total * 100.0)
}

/// The cgroup's headroom, %: `None` without a limit (`max`).
pub(crate) fn cgroup_free_pct(max: &str, current: &str) -> Option<f64> {
    let max: f64 = max.trim().parse().ok()?;
    let current: f64 = current.trim().parse().ok()?;
    (max > 0.0).then(|| ((max - current).max(0.0)) / max * 100.0)
}

#[cfg(test)]
mod tests;
