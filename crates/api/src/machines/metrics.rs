//! `GET /machines/metrics` (decision 0072, point 13, step 7): the series of
//! Settings → Machines' charts, read from Mimir by five fixed queries for
//! every machine at once, for the machines not retired. `range` is `1h`,
//! `24h` (the default) or `7d`, nothing else; each series has a value for
//! each point of the window (`start + k × step` up to `end`), `null` where
//! Mimir had none, so a chart breaks there. Mimir not answering in 5 s is
//! `available: false`, not an error.

use std::collections::BTreeMap;

use application::{MachineMetrics, MachinesService};
use auth_gateway::{Line, MachineCharts, SeriesRange, SeriesWindow};
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::{error, invalid};

/// Which range.
#[derive(Debug, Deserialize, IntoParams)]
pub struct MetricsQuery {
    /// `1h` (a point a minute), `24h` (every 10 minutes, the default) or
    /// `7d` (every hour).
    #[param(example = "24h")]
    range: Option<String>,
}

/// One labelled line of a machine: a filesystem by its watched path's name.
#[derive(Debug, Serialize, ToSchema)]
pub struct LabelledLineOut {
    /// The watched path's name (`docker`, `drive1`…).
    label: String,
    /// A value for each point, `null` where there is none.
    values: Vec<Option<f64>>,
}

/// One machine's series; a list is empty when Mimir has none of it.
#[derive(Debug, Serialize, ToSchema)]
pub struct MachineSeriesOut {
    /// The machine's name (`vm`).
    name: String,
    /// Its last host sample within the range, in Unix epoch seconds.
    last_sample: Option<i64>,
    /// CPU used, %.
    cpu: Vec<Option<f64>>,
    /// Memory used, %.
    memory: Vec<Option<f64>>,
    /// Each watched filesystem's used %.
    filesystems: Vec<LabelledLineOut>,
    /// Bytes received a second.
    network_in: Vec<Option<f64>>,
    /// Bytes sent a second.
    network_out: Vec<Option<f64>>,
}

/// The charts' series.
#[derive(Debug, Serialize, ToSchema)]
pub struct MachineMetricsOut {
    /// `false` when Mimir did not answer in time, or is not read here: no
    /// machine is listed, and the charts say "metrics not available".
    available: bool,
    /// `1h`, `24h` or `7d`.
    range: String,
    /// Seconds between two points.
    step: i64,
    /// The first point, in Unix epoch seconds.
    start: i64,
    /// The last point, in Unix epoch seconds.
    end: i64,
    /// By name; only machines not retired that Mimir has series of.
    machines: Vec<MachineSeriesOut>,
}

/// `GET /machines/metrics`: the machines' charts of one range.
#[utoipa::path(
    get,
    path = "/machines/metrics",
    params(MetricsQuery),
    responses(
        (status = 200, description = "The series, or available false", body = MachineMetricsOut),
        (status = 401, description = "No valid session"),
        (status = 403, description = "Without admin:ManageMachines"),
        (status = 422, description = "A range other than 1h, 24h or 7d"),
    ),
    tag = "machines",
)]
pub async fn metrics_handler(
    State(machines): State<MachinesService>,
    Query(query): Query<MetricsQuery>,
) -> Response {
    let raw = query.range.as_deref().unwrap_or("24h");
    let Some(range) = SeriesRange::parse(raw) else {
        return invalid("range", texts::tr!("api.machines.invalidRange"));
    };
    match machines.machine_metrics(range).await {
        Ok(MachineMetrics::Available(charts)) => Json(available(charts)).into_response(),
        Ok(MachineMetrics::Unavailable(window)) => {
            Json(out(window, false, Vec::new())).into_response()
        }
        Err(e) => error(e),
    }
}

fn out(
    window: SeriesWindow,
    available: bool,
    machines: Vec<MachineSeriesOut>,
) -> MachineMetricsOut {
    MachineMetricsOut {
        available,
        range: window.range.as_str().to_string(),
        step: window.range.step(),
        start: window.start,
        end: window.end,
        machines,
    }
}

/// The lines gathered by machine.
fn available(charts: MachineCharts) -> MachineMetricsOut {
    let mut by: BTreeMap<String, MachineSeriesOut> = BTreeMap::new();
    for Line { vm, values, .. } in charts.cpu {
        entry(&mut by, &vm).cpu = values;
    }
    for Line { vm, values, .. } in charts.memory {
        entry(&mut by, &vm).memory = values;
    }
    for Line { vm, label, values } in charts.filesystems {
        let line = LabelledLineOut { label, values };
        entry(&mut by, &vm).filesystems.push(line);
    }
    for Line { vm, label, values } in charts.network {
        match label.as_str() {
            "in" => entry(&mut by, &vm).network_in = values,
            "out" => entry(&mut by, &vm).network_out = values,
            _ => {}
        }
    }
    for (vm, at) in charts.last_sample {
        entry(&mut by, &vm).last_sample = Some(at);
    }
    out(charts.window, true, by.into_values().collect())
}

/// The machine `vm`'s series, made empty the first time.
fn entry<'a>(by: &'a mut BTreeMap<String, MachineSeriesOut>, vm: &str) -> &'a mut MachineSeriesOut {
    by.entry(vm.to_string())
        .or_insert_with(|| MachineSeriesOut {
            name: vm.to_string(),
            last_sample: None,
            cpu: Vec::new(),
            memory: Vec::new(),
            filesystems: Vec::new(),
            network_in: Vec::new(),
            network_out: Vec::new(),
        })
}
