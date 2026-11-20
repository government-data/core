//! `qd_machine_approved{vm, role}` in the API's metrics file (decision
//! 0072, point 4): 1 for each approved or draining machine, so that Grafana
//! knows which machines should report (`machine_silent`). The machines come
//! from their use case (`MachinesService::list`, over the machines port),
//! read under the main tenant, whose rows are the installation's; a pending
//! or retired machine is never a series. Its two labels are the machine's
//! name, an administrator's label checked when it joined, and its role.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use auth_gateway::{Machine, MachineState};
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;

type Labels = [(&'static str, String); 2];

/// The gauge, and the machines it was last set for.
#[derive(Debug, Clone, Default)]
pub(crate) struct MachineGauge {
    approved: Family<Labels, Gauge>,
    last: Arc<Mutex<HashSet<Labels>>>,
}

impl MachineGauge {
    /// The gauge, registered in the API's `registry`.
    pub(crate) fn register(registry: &mut Registry) -> Self {
        let gauge = Self::default();
        registry.register(
            "qd_machine_approved",
            "1 for each approved or draining machine, by its name and role.",
            gauge.approved.clone(),
        );
        gauge
    }

    /// Every approved or draining machine of `machines` set to 1; a machine
    /// that was and no longer is (retired, renamed away) removed, after the
    /// new ones are set, so a file written meanwhile never lacks one that
    /// stays.
    pub(crate) fn set(&self, machines: &[Machine]) {
        let now: HashSet<Labels> = machines
            .iter()
            .filter(|m| should_report(m.state))
            .map(|m| {
                [
                    ("vm", m.name.clone()),
                    ("role", m.role.as_str().to_string()),
                ]
            })
            .collect();
        for labels in &now {
            self.approved.get_or_create(labels).set(1);
        }
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        for gone in last.difference(&now) {
            self.approved.remove(gone);
        }
        *last = now;
    }
}

/// Whether a machine in `state` should report its host's metrics: approved,
/// and draining, which still runs (point 4).
fn should_report(state: MachineState) -> bool {
    match state {
        MachineState::Approved | MachineState::Draining => true,
        MachineState::Pending | MachineState::Retired => false,
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use auth_gateway::{Machine, MachineId, MachineRole, MachineState};
    use prometheus_client::registry::Registry;

    use super::MachineGauge;

    fn machine(name: &str, role: MachineRole, state: MachineState) -> Machine {
        Machine {
            id: MachineId::new(format!("id-{name}")),
            name: name.into(),
            role,
            pool: Some("pool-a".into()),
            state,
            version: Some("1.0.0".into()),
            cert_serial: Some("0a1b".into()),
            cert_not_after: None,
            cert_serial_next: None,
            cert_not_after_next: None,
            created_at: SystemTime::UNIX_EPOCH,
            approved_at: None,
            approved_by: None,
            retired_at: None,
            retired_by: None,
        }
    }

    /// The file's series, as (vm, role), each checked to be the gauge with
    /// exactly those two labels and the value 1.
    fn series(registry: &Registry) -> Vec<(String, String)> {
        let mut text = String::new();
        prometheus_client::encoding::text::encode(&mut text, registry).unwrap();
        let mut found: Vec<_> = logging::metrics::samples(&text)
            .unwrap()
            .into_iter()
            .map(|s| {
                assert_eq!(s.name, "qd_machine_approved", "{s:?}");
                assert_eq!(s.labels.len(), 2, "{s:?}");
                assert!((s.value - 1.0).abs() < f64::EPSILON, "{s:?}");
                let vm = s.label("vm").unwrap().to_string();
                (vm, s.label("role").unwrap().to_string())
            })
            .collect();
        found.sort();
        found
    }

    fn pair(vm: &str, role: &str) -> (String, String) {
        (vm.into(), role.into())
    }

    #[test]
    fn the_file_names_only_approved_and_draining_machines_by_name_and_role() {
        let mut registry = Registry::default();
        let gauge = MachineGauge::register(&mut registry);
        gauge.set(&[
            machine("worker-1", MachineRole::Worker, MachineState::Approved),
            machine("lake-a", MachineRole::Lake, MachineState::Draining),
            machine("app-1", MachineRole::App, MachineState::Approved),
            machine("worker-2", MachineRole::Worker, MachineState::Pending),
            machine("replica-1", MachineRole::LakeReplica, MachineState::Retired),
        ]);
        assert_eq!(
            series(&registry),
            [
                pair("app-1", "app"),
                pair("lake-a", "lake"),
                pair("worker-1", "worker")
            ]
        );
    }

    #[test]
    fn a_machine_no_longer_approved_leaves_the_file() {
        let mut registry = Registry::default();
        let gauge = MachineGauge::register(&mut registry);
        gauge.set(&[
            machine("worker-1", MachineRole::Worker, MachineState::Approved),
            machine("trino-1", MachineRole::TrinoWorker, MachineState::Approved),
        ]);
        gauge.set(&[
            machine("worker-1", MachineRole::Worker, MachineState::Retired),
            machine("trino-1", MachineRole::TrinoWorker, MachineState::Draining),
        ]);
        assert_eq!(series(&registry), [pair("trino-1", "trino-worker")]);
        gauge.set(&[]);
        assert!(series(&registry).is_empty());
    }
}
