//! Grafana's deliveries (0072, points 11 and 12, step 5): taken from the
//! application VM's machine alone; a firing alert kept once, removed when
//! resolved; an unknown or retired machine changes nothing; the heartbeat
//! handled first, a `DatasourceError` of its rule too; a `Datasource*`
//! alert with no `vm` a row with no machine.

use std::collections::BTreeMap;

use auth_gateway::{AlertChange, AlertKey, FiringAlert, MachineAlertStore};
use serde_json::{json, Map, Value};

use super::*;

const MINUTE: Duration = Duration::from_secs(60);

/// The rows in memory, as the table keeps them: an alert and when it was
/// last written; the heartbeat's time apart.
#[derive(Default)]
struct Rows {
    alerts: Mutex<Vec<(FiringAlert, SystemTime)>>,
    heartbeat: Mutex<Option<SystemTime>>,
    deliveries: AtomicU64,
}

#[async_trait]
impl MachineAlertStore for Rows {
    async fn apply(&self, changes: &[AlertChange], at: SystemTime) -> Result<(), RepositoryError> {
        self.deliveries.fetch_add(1, Ordering::SeqCst);
        let mut alerts = self.alerts.lock().unwrap();
        for change in changes {
            match change {
                AlertChange::Firing(alert) => {
                    let same = alerts.iter().position(|(a, _)| a.key == alert.key);
                    match same {
                        Some(i) if alerts[i].0 == *alert => {}
                        Some(i) => alerts[i] = (alert.clone(), at),
                        None => alerts.push((alert.clone(), at)),
                    }
                }
                AlertChange::Resolved(key) => alerts.retain(|(a, _)| a.key != *key),
                AlertChange::Heartbeat { .. } => *self.heartbeat.lock().unwrap() = Some(at),
            }
        }
        Ok(())
    }
}

pub(super) fn machine(name: &str, role: MachineRole, state: MachineState) -> Machine {
    let at = SystemTime::UNIX_EPOCH;
    Machine {
        id: MachineId::new(format!("m-{name}")),
        name: name.into(),
        role,
        pool: None,
        state,
        version: None,
        cert_serial: Some("0a".into()),
        cert_not_after: Some(at + DAY),
        cert_serial_next: None,
        cert_not_after_next: None,
        created_at: at,
        approved_at: None,
        approved_by: None,
        retired_at: None,
        retired_by: None,
    }
}

/// The application VM's machine, a lake node and a retired one.
fn with_alerts() -> (World, Arc<Rows>) {
    let memory = Memory::default();
    memory.machines.lock().unwrap().extend([
        machine("qd-app", MachineRole::App, Approved),
        machine("lake-a", MachineRole::Lake, Approved),
        machine("old-1", Worker, Retired),
    ]);
    let audit = Arc::new(Audit::default());
    let clock = Arc::new(FixedClock(Mutex::new(SystemTime::UNIX_EPOCH)));
    let rows = Arc::new(Rows::default());
    let service = MachinesService::new(
        Arc::new(memory),
        Arc::new(Mint::default()),
        audit.clone(),
        clock.clone(),
    )
    .with_machine_alerts(rows.clone());
    let w = World {
        service,
        audit,
        clock,
    };
    (w, rows)
}

fn alert(firing: bool, fingerprint: &str, labels: &[(&str, &str)]) -> DeliveredAlert {
    let labels: BTreeMap<String, String> = labels
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let values: Map<String, Value> = json!({ "A": 0.07, "C": 1, "B": "text" })
        .as_object()
        .cloned()
        .unwrap();
    DeliveredAlert {
        firing,
        labels,
        fingerprint: fingerprint.into(),
        starts_at: SystemTime::UNIX_EPOCH + MINUTE,
        values,
    }
}

fn disk(firing: bool, vm: &str) -> DeliveredAlert {
    let labels = [
        ("alertname", "disk_low"),
        ("qd_alert", "disk_low"),
        ("severity", "warning"),
        ("vm", vm),
        ("mountpoint", "drive1"),
    ];
    alert(firing, "0a1b2c3d4e5f6789", &labels)
}

fn heartbeat(alertname: &str) -> DeliveredAlert {
    let labels = [("alertname", alertname), ("qd_alert", "alerting_heartbeat")];
    alert(true, "aaaa0000bbbb1111", &labels)
}

fn unavailable(firing: bool) -> DeliveredAlert {
    let labels = [
        ("alertname", "DatasourceError"),
        ("rulename", "disk_low_worker"),
        ("qd_alert", "disk_low"),
        ("severity", "warning"),
    ];
    alert(firing, "ffee000011112222", &labels)
}

fn qd_app() -> auth_gateway::MachineCertificate {
    presented("app", "qd-app", "0a")
}

async fn deliver(w: &World, alerts: &[DeliveredAlert]) -> AlertsApplied {
    *w.clock.0.lock().unwrap() += MINUTE;
    let o = origin();
    w.service
        .receive_alerts(&qd_app(), alerts, &o)
        .await
        .unwrap()
}

fn kept(rows: &Rows) -> Vec<(FiringAlert, SystemTime)> {
    rows.alerts.lock().unwrap().clone()
}

#[tokio::test]
async fn alerts_delivered_twice_change_nothing_the_second_time_and_resolve_away() {
    let (w, rows) = with_alerts();
    let applied = deliver(&w, &[disk(true, "lake-a")]).await;
    assert_eq!(
        applied,
        AlertsApplied {
            applied: 1,
            unknown: 0
        }
    );
    let first = kept(&rows);
    assert_eq!(first.len(), 1);
    let (alert, _) = &first[0];
    let key = AlertKey::Machine {
        machine: MachineId::new("m-lake-a"),
        fingerprint: "0a1b2c3d4e5f6789".into(),
    };
    assert_eq!((&alert.key, alert.qd_alert.as_str()), (&key, "disk_low"));
    assert_eq!(alert.severity, Some(auth_gateway::AlertSeverity::Warning));
    let values = json!({ "A": 0.07, "C": 1, "path": "drive1" });
    assert_eq!(
        Value::Object(alert.values.clone()),
        values,
        "numbers and path only"
    );

    deliver(&w, &[disk(true, "lake-a")]).await;
    assert_eq!(kept(&rows), first, "the second time changes nothing");

    deliver(&w, &[disk(false, "lake-a")]).await;
    assert!(kept(&rows).is_empty(), "resolved: removed");
    assert!(
        w.audit.0.lock().unwrap().is_empty(),
        "a delivery is not audited"
    );
}

#[tokio::test]
async fn alerts_of_an_unknown_or_retired_machine_change_nothing() {
    let (w, rows) = with_alerts();
    let mut nameless = disk(true, "x");
    nameless.labels.remove("vm");
    let mut no_key = disk(true, "lake-a");
    no_key.labels.remove("qd_alert");
    let left_out = [disk(true, "lake-z"), disk(true, "old-1"), nameless, no_key];
    let applied = deliver(&w, &left_out).await;
    assert_eq!(
        applied,
        AlertsApplied {
            applied: 0,
            unknown: 4
        }
    );
    assert!(kept(&rows).is_empty());
    assert!(rows.heartbeat.lock().unwrap().is_none());
}

#[tokio::test]
async fn alerts_are_taken_from_the_application_vms_machine_alone() {
    let (w, rows) = with_alerts();
    for other in [
        presented("worker", "qd-app", "0a"),
        presented("lake", "lake-a", "0a"),
        presented("app", "qd-app", "0b"),
    ] {
        let (alerts, o) = ([disk(true, "lake-a")], origin());
        let refused = w.service.receive_alerts(&other, &alerts, &o).await;
        assert_eq!(refused, Err(AlertsError::Refused));
    }
    assert_eq!(rows.deliveries.load(Ordering::SeqCst), 0, "nothing written");
    let audit = w.audit.0.lock().unwrap();
    let last = audit.last().unwrap();
    assert_eq!(last.event_type, AuditEventType::CertificateRefused);
    assert_eq!(last.action, "alerts:certificate");
}

#[tokio::test]
async fn alerts_need_a_store() {
    let w = world();
    let refused = w.service.receive_alerts(&qd_app(), &[], &origin()).await;
    assert_eq!(refused, Err(AlertsError::Unavailable));
}

#[tokio::test]
async fn the_heartbeat_takes_every_delivery_and_its_datasource_error_counts_as_alive() {
    let (w, rows) = with_alerts();
    deliver(&w, &[heartbeat("alerting_heartbeat")]).await;
    let first = rows.heartbeat.lock().unwrap().unwrap();
    deliver(&w, &[heartbeat("alerting_heartbeat")]).await;
    let second = rows.heartbeat.lock().unwrap().unwrap();
    assert_eq!(
        second,
        first + MINUTE,
        "received_at updated the second time too"
    );

    let applied = deliver(&w, &[heartbeat("DatasourceError")]).await;
    assert_eq!(
        applied,
        AlertsApplied {
            applied: 1,
            unknown: 0
        }
    );
    assert_eq!(rows.heartbeat.lock().unwrap().unwrap(), second + MINUTE);
    assert!(
        kept(&rows).is_empty(),
        "no other row: alive, not unavailable"
    );
    assert!(w.audit.0.lock().unwrap().is_empty(), "no event, no audit");
}

#[tokio::test]
async fn a_datasource_error_with_no_vm_is_one_machineless_row_until_resolved() {
    let (w, rows) = with_alerts();
    deliver(&w, &[unavailable(true)]).await;
    deliver(&w, &[unavailable(true)]).await;
    let rows_now = kept(&rows);
    assert_eq!(rows_now.len(), 1, "one row");
    let key = AlertKey::Machineless {
        alertname: "DatasourceError".into(),
        rule_name: "disk_low_worker".into(),
    };
    assert_eq!(rows_now[0].0.key, key);
    deliver(&w, &[unavailable(false)]).await;
    assert!(kept(&rows).is_empty(), "its resolution removes it");
}
