//! The machines' charts (0072, point 13, step 7): only machines not retired
//! are charted; Mimir not answering, or not read at all, is
//! `Unavailable` with the range's window, never an error.

use auth_gateway::{Line, MachineCharts, MachineSeries, SeriesRange, SeriesUnavailable};

use super::alerts::machine;
use super::*;

/// Mimir as a test: an answer for every machine named, or none.
struct Mimir {
    answers: bool,
    asked: Mutex<Vec<(SeriesRange, SystemTime)>>,
}

#[async_trait]
impl MachineSeries for Mimir {
    async fn read(
        &self,
        range: SeriesRange,
        now: SystemTime,
    ) -> Result<MachineCharts, SeriesUnavailable> {
        self.asked.lock().unwrap().push((range, now));
        if !self.answers {
            return Err(SeriesUnavailable("connection refused".into()));
        }
        let window = range.window(now);
        let line = |vm: &str, label: &str| Line {
            vm: vm.into(),
            label: label.into(),
            values: vec![Some(1.0); range.points()],
        };
        let vms = ["worker-1", "old-1", "never-joined"];
        Ok(MachineCharts {
            window,
            cpu: vms.iter().map(|vm| line(vm, "")).collect(),
            memory: vms.iter().map(|vm| line(vm, "")).collect(),
            filesystems: vms.iter().map(|vm| line(vm, "docker")).collect(),
            network: vms.iter().map(|vm| line(vm, "in")).collect(),
            last_sample: vms.iter().map(|vm| (vm.to_string(), window.end)).collect(),
        })
    }
}

fn charted(answers: bool) -> (World, Arc<Mimir>) {
    let memory = Memory::default();
    memory.machines.lock().unwrap().extend([
        machine("worker-1", Worker, Approved),
        machine("old-1", Worker, Retired),
    ]);
    let audit = Arc::new(Audit::default());
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let clock = Arc::new(FixedClock(Mutex::new(now)));
    let mimir = Arc::new(Mimir {
        answers,
        asked: Mutex::default(),
    });
    let service = MachinesService::new(
        Arc::new(memory),
        Arc::new(Mint::default()),
        audit.clone(),
        clock.clone(),
    )
    .with_machine_series(mimir.clone());
    let w = World {
        service,
        audit,
        clock,
    };
    (w, mimir)
}

#[tokio::test]
async fn only_machines_not_retired_are_charted() {
    let (w, mimir) = charted(true);
    let got = w.service.machine_metrics(SeriesRange::Day).await.unwrap();
    let MachineMetrics::Available(charts) = got else {
        panic!("expected the charts, got {got:?}");
    };
    let vms = |lines: &[Line]| lines.iter().map(|l| l.vm.clone()).collect::<Vec<_>>();
    for lines in [
        &charts.cpu,
        &charts.memory,
        &charts.filesystems,
        &charts.network,
    ] {
        assert_eq!(vms(lines), ["worker-1"]);
    }
    let last: Vec<_> = charts
        .last_sample
        .iter()
        .map(|(vm, _)| vm.as_str())
        .collect();
    assert_eq!(last, ["worker-1"]);
    let asked = mimir.asked.lock().unwrap().clone();
    assert_eq!(asked, [(SeriesRange::Day, *w.clock.0.lock().unwrap())]);
    assert!(w.audit.0.lock().unwrap().is_empty(), "a read, not audited");
}

#[tokio::test]
async fn mimir_down_is_unavailable_with_the_ranges_window() {
    let (w, _) = charted(false);
    let got = w.service.machine_metrics(SeriesRange::Hour).await.unwrap();
    let now = *w.clock.0.lock().unwrap();
    assert_eq!(
        got,
        MachineMetrics::Unavailable(SeriesRange::Hour.window(now))
    );
}

#[tokio::test]
async fn without_mimir_the_charts_are_unavailable() {
    let w = world();
    let got = w.service.machine_metrics(SeriesRange::Week).await.unwrap();
    assert!(matches!(got, MachineMetrics::Unavailable(win) if win.range == SeriesRange::Week));
}
