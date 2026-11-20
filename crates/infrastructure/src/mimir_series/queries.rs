//! The five queries of decision 0072, point 13, as V4
//! (`tools/verify/mimir_query_cert.sh`) ran them: each one for every
//! machine at once, grouped by `vm`. They are fixed strings; the only
//! thing a range changes is the rate's window and the last sample's
//! lookback, both from the closed set of [`SeriesRange`]. Nothing a client
//! sends reaches a query.

use auth_gateway::{Line, SeriesRange, SeriesWindow};
use serde_json::Value;

/// CPU used, %.
const CPU: &str =
    r#"100 * (1 - avg by (vm) (rate(node_cpu_seconds_total{job="host",mode="idle"}[$window])))"#;
/// Memory used, %.
const MEMORY: &str = r#"100 * (1 - sum by (vm) (node_memory_MemAvailable_bytes{job="host"}) / sum by (vm) (node_memory_MemTotal_bytes{job="host"}))"#;
/// Each watched filesystem's used %, by its name (`mountpoint`).
const FILESYSTEMS: &str = r#"100 * (1 - max by (vm, mountpoint) (node_filesystem_avail_bytes{job="host"}) / max by (vm, mountpoint) (node_filesystem_size_bytes{job="host"}))"#;
/// Bytes a second in and out (`direction`).
const NETWORK: &str = r#"sum by (vm, direction) (label_replace(rate(node_network_receive_bytes_total{job="host"}[$window]), "direction", "in", "", "") or label_replace(rate(node_network_transmit_bytes_total{job="host"}[$window]), "direction", "out", "", ""))"#;
/// Each machine's last host sample within the range: a subquery, as V4
/// showed a plain `timestamp()` drops a machine silent past the lookback.
const LAST_SAMPLE: &str =
    r#"max by (vm) (max_over_time(timestamp(node_boot_time_seconds{job="host"})[$range:1m]))"#;

/// The rate's window: two scrapes (30 s) at least, and the step beyond.
fn window(range: SeriesRange) -> &'static str {
    match range {
        SeriesRange::Hour => "2m",
        SeriesRange::Day => "10m",
        SeriesRange::Week => "1h",
    }
}

/// The four ranged queries: CPU, memory, filesystems, network.
pub(crate) fn ranged(range: SeriesRange) -> [String; 4] {
    [CPU, MEMORY, FILESYSTEMS, NETWORK].map(|q| q.replace("$window", window(range)))
}

/// The last sample's instant query.
pub(crate) fn last_sample(range: SeriesRange) -> String {
    LAST_SAMPLE.replace("$range", range.as_str())
}

/// A `query_range` answer as lines on `window`'s points; `label` names the
/// label that tells a machine's lines apart (`mountpoint`, `direction`), if
/// any. `None` if it is not Prometheus's answer.
pub(crate) fn lines(
    answer: &Value,
    window: &SeriesWindow,
    label: Option<&str>,
) -> Option<Vec<Line>> {
    let mut lines = Vec::new();
    for series in result(answer, "matrix")? {
        let metric = series.get("metric")?;
        let Some(vm) = metric.get("vm").and_then(Value::as_str) else {
            continue;
        };
        let label = match label {
            Some(name) => metric.get(name).and_then(Value::as_str)?.to_string(),
            None => String::new(),
        };
        let mut values = vec![None; window.range.points()];
        for point in series.get("values")?.as_array()? {
            let (t, v) = sample(point)?;
            if let (Some(k), Some(v)) = (window.index(t), v) {
                values[k] = Some(v);
            }
        }
        lines.push(Line {
            vm: vm.to_string(),
            label,
            values,
        });
    }
    lines.sort_by(|a, b| (&a.vm, &a.label).cmp(&(&b.vm, &b.label)));
    Some(lines)
}

/// An instant `query` answer: each machine's value as whole seconds.
pub(crate) fn last_samples(answer: &Value) -> Option<Vec<(String, i64)>> {
    let mut last = Vec::new();
    for series in result(answer, "vector")? {
        let vm = series.get("metric")?.get("vm").and_then(Value::as_str);
        let (_, v) = sample(series.get("value")?)?;
        if let (Some(vm), Some(v)) = (vm, v) {
            last.push((vm.to_string(), v as i64));
        }
    }
    last.sort();
    Some(last)
}

fn result<'a>(answer: &'a Value, kind: &str) -> Option<&'a Vec<Value>> {
    let ok = answer.get("status")? == "success";
    let data = answer.get("data")?;
    (ok && data.get("resultType")? == kind).then_some(())?;
    data.get("result")?.as_array()
}

/// `[t, "v"]`: the time in whole seconds and the value, `None` if it is not
/// a finite number (`NaN`, `+Inf`).
fn sample(point: &Value) -> Option<(i64, Option<f64>)> {
    let pair = point.as_array()?;
    let t = pair.first()?.as_f64()?;
    let v = pair.get(1)?.as_str()?.parse::<f64>().ok();
    Some((t.round() as i64, v.filter(|v| v.is_finite())))
}
