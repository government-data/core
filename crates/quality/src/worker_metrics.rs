//! What a worker counts of the jobs it takes from the queue (decision 0071,
//! point 8: "Is it working, how fast, how often does it fail?"):
//! `qd_worker_slots`, `qd_worker_jobs_running`, `qd_worker_jobs_total` by
//! kind and result and `qd_worker_job_duration_seconds` by kind, and
//! `qd_worker_claim_paused` by reason (`disk`, `memory`: decision 0072,
//! point 14, 1 while the worker claims nothing at that limit), in the
//! registry the worker writes to its metrics file. Beside the queue's own
//! gauges ([`crate::queue_ready`]), with the same list of kinds.
//!
//! A job is labelled by its **kind**, one of [`KINDS`], and its **result**,
//! one of [`JobResult`]'s: never its dataset, source, cut, tenant or id
//! (point 10).

use std::time::Instant;

use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::registry::Registry;

use crate::queue_ready::KINDS;

/// The kind of a job whose kind is not in the queue's list (none should be).
pub const OTHER: &str = "other";

/// A job's durations' buckets, in seconds: from a listing to an hour's run.
const BUCKETS: [f64; 9] = [0.5, 1.0, 5.0, 15.0, 60.0, 300.0, 900.0, 1800.0, 3600.0];

/// How a job this worker took ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobResult {
    /// Done.
    Ok,
    /// Failed, or could not be finished.
    Failed,
    /// Not this worker's to finish: put back (Trino held) or its lease lost.
    Cancelled,
}

impl JobResult {
    /// Every result, the fixed list the label takes its value from.
    pub const ALL: [Self; 3] = [Self::Ok, Self::Failed, Self::Cancelled];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

type Jobs = Family<[(&'static str, &'static str); 2], Counter>;
type Paused = Family<[(&'static str, &'static str); 1], Gauge>;
type Durations = Family<[(&'static str, &'static str); 1], Histogram, fn() -> Histogram>;

fn histogram() -> Histogram {
    Histogram::new(BUCKETS)
}

/// The worker's counters, cheap to clone (each is shared).
#[derive(Debug, Clone)]
pub struct WorkerMetrics {
    slots: Gauge,
    running: Gauge,
    jobs: Jobs,
    durations: Durations,
    paused: Paused,
}

/// A job being run: counted as running until [`WorkerMetrics::finished`].
#[derive(Debug)]
pub struct Running {
    kind: &'static str,
    since: Instant,
}

/// The kind as a label: the queue's own `&'static str`, or [`OTHER`].
pub fn kind_label(kind: &str) -> &'static str {
    KINDS.iter().copied().find(|k| *k == kind).unwrap_or(OTHER)
}

impl WorkerMetrics {
    pub fn register(registry: &mut Registry, slots: usize) -> Self {
        let metrics = Self {
            slots: Gauge::default(),
            running: Gauge::default(),
            jobs: Jobs::default(),
            durations: Durations::new_with_constructor(histogram),
            paused: Paused::default(),
        };
        metrics.slots.set(i64::try_from(slots).unwrap_or(i64::MAX));
        registry.register(
            "qd_worker_slots",
            "Jobs this worker runs at a time.",
            metrics.slots.clone(),
        );
        registry.register(
            "qd_worker_jobs_running",
            "Jobs this worker is running now.",
            metrics.running.clone(),
        );
        registry.register(
            "qd_worker_jobs",
            "Jobs this worker took, by kind and how they ended.",
            metrics.jobs.clone(),
        );
        registry.register(
            "qd_worker_job_duration_seconds",
            "How long this worker's jobs took, by kind, in seconds.",
            metrics.durations.clone(),
        );
        registry.register(
            "qd_worker_claim_paused",
            "1 while this worker claims no job at its disk or memory limit, by reason.",
            metrics.paused.clone(),
        );
        metrics
    }

    /// Which of the worker's limits stop its claims now.
    pub fn set_claim_paused(&self, disk: bool, memory: bool) {
        for (reason, on) in [("disk", disk), ("memory", memory)] {
            self.paused
                .get_or_create(&[("reason", reason)])
                .set(i64::from(on));
        }
    }

    pub fn started(&self, kind: &str) -> Running {
        self.running.inc();
        Running {
            kind: kind_label(kind),
            since: Instant::now(),
        }
    }

    pub fn finished(&self, job: Running, result: JobResult) {
        self.running.dec();
        self.jobs
            .get_or_create(&[("kind", job.kind), ("result", result.as_str())])
            .inc();
        self.durations
            .get_or_create(&[("kind", job.kind)])
            .observe(job.since.elapsed().as_secs_f64());
    }
}

#[cfg(test)]
mod tests;
