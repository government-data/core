//! The metrics file every Rust service writes (decision 0071, point 9): the
//! API, the worker and the node agent each keep their counters in a
//! `prometheus-client` [`Registry`] and, every 15 seconds, write it in the
//! text format to `<dir>/<service>.prom`, which the machine's Alloy reads
//! with its `textfile` collector. No port, no network.
//!
//! The file is written aside (`.<service>.prom.tmp`, a name the collector
//! skips) and renamed over the last one, so a reader never sees half of it;
//! it is left `0644`, readable by Alloy's user. A missing or read-only
//! directory never stops the service: [`MetricsFile::tick`] logs one warning
//! when writing starts failing and one line when it works again, nothing on
//! each tick in between.
//!
//! The directory is `QD_METRICS_DIR` ([`DEFAULT_DIR`] when unset); `off`
//! writes nothing. Each file also carries the process's own two series,
//! `process_cpu_seconds_total` and `process_resident_memory_bytes`, with a
//! `service` label, since two services on one machine share Alloy's folder.
//!
//! What goes in a registry is each service's own: a label takes a value
//! from a fixed list (a route's template, a job's kind), never a tenant's,
//! a person's or a path. [`samples`] reads a file back, for the tests that
//! check it.

mod process;
mod read;

#[cfg(test)]
mod tests;

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use prometheus_client::registry::Registry;

pub use read::{samples, Sample};

/// Where the files go when `QD_METRICS_DIR` is not set: the volume each
/// service shares with Alloy.
pub const DEFAULT_DIR: &str = "/run/qd-metrics";

/// How often the file is written.
pub const EVERY: Duration = Duration::from_secs(15);

type Refresh = Box<dyn Fn() + Send + Sync>;

/// One service's metrics file.
pub struct MetricsFile {
    path: PathBuf,
    aside: PathBuf,
    registry: Registry,
    refresh: Option<Refresh>,
    failing: AtomicBool,
}

impl std::fmt::Debug for MetricsFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetricsFile")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl MetricsFile {
    /// The file of `service` in `dir`, with its registry; the process's own
    /// series are added to it.
    pub fn new(service: &str, dir: impl Into<PathBuf>, mut registry: Registry) -> Self {
        let dir = dir.into();
        registry.register_collector(Box::new(process::Process::new(service)));
        Self {
            path: dir.join(format!("{service}.prom")),
            aside: dir.join(format!(".{service}.prom.tmp")),
            registry,
            refresh: None,
            failing: AtomicBool::new(false),
        }
    }

    /// As [`MetricsFile::new`], in `QD_METRICS_DIR` (or [`DEFAULT_DIR`]);
    /// `None` when it is `off`.
    pub fn from_env(service: &str, registry: Registry) -> Option<Self> {
        let dir = std::env::var("QD_METRICS_DIR").unwrap_or_else(|_| DEFAULT_DIR.into());
        let dir = dir.trim();
        if dir.eq_ignore_ascii_case("off") {
            tracing::info!("QD_METRICS_DIR is off: no metrics file");
            return None;
        }
        let dir = if dir.is_empty() { DEFAULT_DIR } else { dir };
        Some(Self::new(service, dir, registry))
    }

    /// Something to run before each write, to set the gauges read from
    /// elsewhere (a certificate's date).
    #[must_use]
    pub fn with_refresh(mut self, refresh: impl Fn() + Send + Sync + 'static) -> Self {
        self.refresh = Some(Box::new(refresh));
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file's text as it would be written now.
    pub fn render(&self) -> String {
        if let Some(refresh) = &self.refresh {
            refresh();
        }
        let mut text = String::new();
        // Writing to a String fails only if a metric's own encoding does.
        if let Err(e) = prometheus_client::encoding::text::encode(&mut text, &self.registry) {
            tracing::warn!("metrics: encoding: {e}");
        }
        text
    }

    /// Writes the file aside, readable by everyone, and renames it over the
    /// last one.
    pub fn write(&self) -> io::Result<()> {
        let text = self.render();
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o644)
            .open(&self.aside)?;
        file.write_all(text.as_bytes())?;
        // The mode again, past the process's umask.
        file.set_permissions(fs::Permissions::from_mode(0o644))?;
        drop(file);
        fs::rename(&self.aside, &self.path)
    }

    /// Writes the file; a failure is logged once until it works again, and
    /// never goes further. Says whether it was written.
    pub fn tick(&self) -> bool {
        match self.write() {
            Ok(()) => {
                if self.failing.swap(false, Ordering::Relaxed) {
                    tracing::info!(file = %self.path.display(), "metrics file written again");
                }
                true
            }
            Err(e) => {
                if !self.failing.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        file = %self.path.display(),
                        "metrics file not written, the service goes on without it: {e}"
                    );
                }
                let _ = fs::remove_file(&self.aside);
                false
            }
        }
    }

    /// Writes the file now and every [`EVERY`] on a thread of its own, for
    /// as long as the process runs.
    pub fn spawn(self) {
        let spawned = std::thread::Builder::new()
            .name("metrics-file".into())
            .spawn(move || loop {
                self.tick();
                std::thread::sleep(EVERY);
            });
        if let Err(e) = spawned {
            tracing::warn!("metrics file: no thread for it: {e}");
        }
    }
}
