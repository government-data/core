//! The worker's metrics file (decision 0071, point 9): the counters of
//! `quality::worker_metrics`, which the slots feed, and the process's own,
//! written to `/run/qd-metrics/worker.prom` every 15 seconds
//! (`logging::metrics`).

use logging::metrics::MetricsFile;
use prometheus_client::registry::Registry;
use quality::worker_metrics::WorkerMetrics;

/// The `service` of the file, as its log lines'.
const SERVICE: &str = "worker";

/// The counters, their file written from now on (unless `QD_METRICS_DIR`
/// is `off`).
pub(crate) fn start(slots: usize) -> WorkerMetrics {
    let mut registry = Registry::default();
    let metrics = WorkerMetrics::register(&mut registry, slots);
    if let Some(file) = MetricsFile::from_env(SERVICE, registry) {
        tracing::info!(file = %file.path().display(), "metrics file");
        file.spawn();
    }
    metrics
}
