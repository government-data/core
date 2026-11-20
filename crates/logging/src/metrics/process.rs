//! The process's CPU time and resident memory, read from `/proc/self` at
//! each write (decision 0071, point 8: "How loaded is it?"). Labelled by
//! `service` alone. Where `/proc` cannot be read, nothing is written.

use prometheus_client::collector::Collector;
use prometheus_client::encoding::{DescriptorEncoder, EncodeMetric};
use prometheus_client::metrics::counter::ConstCounter;
use prometheus_client::metrics::gauge::ConstGauge;

/// Linux's clock ticks per second as `/proc` counts them (`USER_HZ`), the
/// same on every architecture the services run on.
const TICKS: f64 = 100.0;

#[derive(Debug)]
pub(super) struct Process {
    service: String,
}

impl Process {
    pub(super) fn new(service: &str) -> Self {
        Self {
            service: service.to_owned(),
        }
    }
}

impl Collector for Process {
    fn encode(&self, mut encoder: DescriptorEncoder) -> Result<(), std::fmt::Error> {
        let labels = [("service", self.service.as_str())];
        if let Some(seconds) = cpu_seconds() {
            let counter = ConstCounter::new(seconds);
            let mut metric = encoder.encode_descriptor(
                "process_cpu_seconds",
                "User and system CPU time the process has used, in seconds.",
                None,
                counter.metric_type(),
            )?;
            counter.encode(metric.encode_family(&labels)?)?;
        }
        if let Some(bytes) = resident_bytes() {
            let gauge = ConstGauge::new(bytes);
            let mut metric = encoder.encode_descriptor(
                "process_resident_memory_bytes",
                "Resident memory of the process, in bytes.",
                None,
                gauge.metric_type(),
            )?;
            gauge.encode(metric.encode_family(&labels)?)?;
        }
        Ok(())
    }
}

/// `utime` plus `stime` of `/proc/self/stat`, in seconds.
fn cpu_seconds() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    cpu_seconds_of(&stat)
}

/// The fields after the command's closing parenthesis start at `state`
/// (field 3): `utime` is field 14 and `stime` field 15.
pub(super) fn cpu_seconds_of(stat: &str) -> Option<f64> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    #[allow(clippy::cast_precision_loss)] // ticks stay far below 2^52
    Some((utime + stime) as f64 / TICKS)
}

/// `VmRSS` of `/proc/self/status`, in bytes.
fn resident_bytes() -> Option<i64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    resident_bytes_of(&status)
}

pub(super) fn resident_bytes_of(status: &str) -> Option<i64> {
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: i64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kb.checked_mul(1024)
}
