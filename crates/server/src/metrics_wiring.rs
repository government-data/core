//! The API's metrics file (decision 0071, points 8 and 9): the requests by
//! route template (`http_kit::metrics`), the queue's ready jobs and the
//! oldest's wait by kind (`quality::queue_ready`, summed over this cell's
//! active tenants), the approved machines (`crate::machine_gauge`, decision
//! 0072) and Grafana's alerts of no known machine ([`unknown_alerts`]),
//! and the process's CPU and memory, written to
//! `/run/qd-metrics/api.prom` every 15 seconds (`logging::metrics`). No
//! use case or domain type sees any of it: the counting is in the
//! middleware, the queue's adapter and here.

use std::sync::LazyLock;

use api::machines::alerts::UnknownAlerts;
use application::MachinesService;
use http_kit::metrics::HttpMetrics;
use logging::metrics::{MetricsFile, EVERY};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::registry::Registry;
use quality::queue_ready::{self, QueueGauges};

use crate::machine_gauge::MachineGauge;

/// The `service` of the file, as its log lines'.
const SERVICE: &str = "api";

/// `qd_machine_alerts_unknown_total` (decision 0072, point 11): the alerts
/// Grafana delivered of a machine unknown or retired. One per process, as
/// the file: the alerts route counts into it ([`unknown_alerts`]) and
/// [`start`] registers it.
static UNKNOWN_ALERTS: LazyLock<Counter> = LazyLock::new(Counter::default);

/// What the alerts route counts its left-out alerts with.
pub(crate) fn unknown_alerts() -> UnknownAlerts {
    UnknownAlerts::new(|n| {
        UNKNOWN_ALERTS.inc_by(n);
    })
}

/// The registry with the request metrics, the queue's gauges and the
/// machines', its file written from now on (unless `QD_METRICS_DIR` is
/// `off`), and the gauges read from the queue and the machines every 15
/// seconds. Returns what the router's middleware counts with.
pub(crate) fn start(pool: infrastructure::PgPool, machines: MachinesService) -> HttpMetrics {
    let mut registry = Registry::default();
    registry.register(
        "qd_machine_alerts_unknown",
        "Alerts Grafana delivered of a machine unknown or retired.",
        UNKNOWN_ALERTS.clone(),
    );
    let http = HttpMetrics::register(&mut registry);
    let queue = QueueGauges::register(&mut registry);
    let approved = MachineGauge::register(&mut registry);
    if let Some(file) = MetricsFile::from_env(SERVICE, registry) {
        tracing::info!(file = %file.path().display(), "metrics file");
        file.spawn();
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(EVERY);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut failing = false;
        let mut machines_failing = false;
        loop {
            interval.tick().await;
            // The installation's machines. Once until it works again, as the
            // queue's.
            match machines.list().await {
                Ok(list) => {
                    approved.set(&list);
                    machines_failing = false;
                }
                Err(e) => {
                    if !machines_failing {
                        tracing::warn!("metrics: reading the machines: {e:?}");
                    }
                    machines_failing = true;
                }
            }
            match read_queue(&pool).await {
                Ok(rows) => {
                    queue.set(&rows);
                    failing = false;
                }
                // Once until it works again: the queue's gauges keep their
                // last values meanwhile.
                Err(e) => {
                    if !failing {
                        tracing::warn!("metrics: reading the queue: {e}");
                    }
                    failing = true;
                }
            }
        }
    });
    http
}

/// The ready jobs.
async fn read_queue(
    pool: &infrastructure::PgPool,
) -> Result<Vec<queue_ready::Ready>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(queue_ready::ready(pool).await?)
}
