//! The machines' charts (decision 0072, point 13, step 7): `GET
//! /machines/metrics`, for whoever may `admin:ManageMachines`. The series
//! are Mimir's, read through [`MachineSeries`] for a range of a closed set,
//! and kept only for machines that are not retired. Mimir not answering,
//! or this API not reading it, is an answer, not an error: the cards keep
//! their state and link, and the charts say "metrics not available". The
//! application never decides anything from these series.

use std::collections::HashSet;
use std::sync::Arc;

use auth_gateway::{Line, MachineCharts, MachineSeries, MachineState, SeriesRange, SeriesWindow};

use super::{MachinesError, MachinesService};

/// What the cards' charts get.
#[derive(Debug, Clone, PartialEq)]
pub enum MachineMetrics {
    /// The series of the machines that are not retired.
    Available(MachineCharts),
    /// Mimir did not answer in time, or this API does not read it.
    Unavailable(SeriesWindow),
}

impl MachinesService {
    /// With the machines' series, read from Mimir.
    pub fn with_machine_series(mut self, series: Arc<dyn MachineSeries>) -> Self {
        self.series = Some(series);
        self
    }

    /// The charts of `range` for the machines not retired.
    pub async fn machine_metrics(
        &self,
        range: SeriesRange,
    ) -> Result<MachineMetrics, MachinesError> {
        let now = self.clock.now();
        let Some(series) = self.series.as_ref() else {
            return Ok(MachineMetrics::Unavailable(range.window(now)));
        };
        let machines = self.registry.list().await?;
        let live: HashSet<String> = machines
            .into_iter()
            .filter(|m| m.state != MachineState::Retired)
            .map(|m| m.name)
            .collect();
        match series.read(range, now).await {
            Ok(charts) => Ok(MachineMetrics::Available(only(charts, &live))),
            Err(e) => {
                tracing::warn!(range = range.as_str(), error = %e.0, "the machines' series are not available");
                Ok(MachineMetrics::Unavailable(range.window(now)))
            }
        }
    }
}

/// The charts of the machines named in `live` alone.
fn only(charts: MachineCharts, live: &HashSet<String>) -> MachineCharts {
    let keep = |lines: Vec<Line>| -> Vec<Line> {
        lines.into_iter().filter(|l| live.contains(&l.vm)).collect()
    };
    MachineCharts {
        window: charts.window,
        cpu: keep(charts.cpu),
        memory: keep(charts.memory),
        filesystems: keep(charts.filesystems),
        network: keep(charts.network),
        last_sample: charts
            .last_sample
            .into_iter()
            .filter(|(vm, _)| live.contains(vm))
            .collect(),
    }
}
