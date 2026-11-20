//! The agent's metrics file (decision 0071, point 8: "Are its certificates
//! renewing?"): `qd_node_renewals_total` by result, one of
//! [`Renewal::ALL`], and `qd_node_certificate_expiry_timestamp_seconds`, when
//! the certificate in use ends (0 before the machine has one), written to
//! `/run/qd-metrics/node-agent.prom` every 15 seconds (`logging::metrics`).
//! The agent is told of each renewal through its port
//! ([`crate::ports::Renewals`]); the date is read from its files before each
//! write. No name, serial or key is a label.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use logging::metrics::MetricsFile;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;

use crate::ports::{Renewal, Renewals};
use crate::store::FileStore;

type ByResult = Family<[(&'static str, &'static str); 1], Counter>;

/// The agent's two metrics.
#[derive(Debug, Clone, Default)]
pub struct NodeMetrics {
    renewals: ByResult,
    expiry: Gauge<f64, AtomicU64>,
}

impl NodeMetrics {
    /// Both, registered in `registry`; every result starts at 0.
    pub fn register(registry: &mut Registry) -> Self {
        let metrics = Self::default();
        for how in Renewal::ALL {
            drop(metrics.renewals.get_or_create(&[("result", how.as_str())]));
        }
        registry.register(
            "qd_node_renewals",
            "Renewals of this machine's certificate, by how they ended.",
            metrics.renewals.clone(),
        );
        registry.register(
            "qd_node_certificate_expiry_timestamp_seconds",
            "When the certificate in use ends, in seconds since 1970; 0 before there is one.",
            metrics.expiry.clone(),
        );
        metrics
    }

    /// The end of the certificate in `store`, if there is one to read.
    pub fn read_expiry(&self, store: &FileStore) {
        let Ok(Some(current)) = store.current() else {
            return;
        };
        if let Ok(validity) = current.validity() {
            if let Ok(since) = validity.not_after.duration_since(UNIX_EPOCH) {
                self.expiry.set(since.as_secs_f64());
            }
        }
    }

    /// The metrics, their file written from now on (unless
    /// `QD_METRICS_DIR` is `off`), the date read from `store` before each
    /// write.
    pub fn start(store: FileStore) -> Arc<Self> {
        let mut registry = Registry::default();
        let metrics = Arc::new(Self::register(&mut registry));
        if let Some(file) = MetricsFile::from_env(crate::SERVICE, registry) {
            tracing::info!(file = %file.path().display(), "metrics file");
            let reading = metrics.clone();
            file.with_refresh(move || reading.read_expiry(&store))
                .spawn();
        }
        metrics
    }
}

impl Renewals for NodeMetrics {
    fn renewed(&self, how: Renewal) {
        self.renewals
            .get_or_create(&[("result", how.as_str())])
            .inc();
    }
}

#[cfg(test)]
mod tests;
