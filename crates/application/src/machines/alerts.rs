//! Grafana's deliveries about machines (decision 0072, points 10 to 12, step
//! 5 as amended on 2026-11-16): `POST /internal/machines/alerts`, with the
//! application VM's certificate (role `app`), in the installation's tenant.
//! Each alert, in order:
//!
//! 1. `qd_alert = alerting_heartbeat`, whatever its status, severity or
//!    `alertname` (a `DatasourceError` too): the heartbeat's row takes the
//!    delivery's time. Alerting is alive; no event.
//! 2. `DatasourceError` or `DatasourceNoData` with no `vm`: a row with no
//!    machine, by `alertname` and the rule's name (metrics not available),
//!    deleted when resolved.
//! 3. A `vm`: the machine of that name that is not retired; firing keeps
//!    the row by its fingerprint, resolved deletes it. An unknown or
//!    retired name changes nothing: one log line with the name and
//!    `qd_alert`, and counted ([`AlertsApplied::unknown`]).
//!
//! The rows of one delivery are written in one transaction. 0025's events
//! (`machine_alert`, `metrics_unavailable` and their resolutions) are step
//! 6's: 0025's outbox is not built, so none is written here.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::SystemTime;

use auth_gateway::{
    AlertChange, AlertKey, AlertSeverity, FiringAlert, MachineAlertStore, MachineCertificate,
    MachineId, MachineRole, ALERTING_HEARTBEAT,
};
use domain::AuditEventType::CertificateRefused;
use serde_json::{Map, Value};

use super::{MachinesService, Origin};

/// The alerts Grafana makes itself when it cannot evaluate a rule.
const DATASOURCE_ALERTS: [&str; 2] = ["DatasourceError", "DatasourceNoData"];
/// The labels a text names, kept in a row's values under their text's name.
const LABELS: [(&str, &str); 2] = [("mountpoint", "path"), ("device", "device")];

/// One alert of a delivery, as Grafana's webhook sends it.
#[derive(Debug, Clone, PartialEq)]
pub struct DeliveredAlert {
    /// `firing`, or resolved.
    pub firing: bool,
    pub labels: BTreeMap<String, String>,
    pub fingerprint: String,
    pub starts_at: SystemTime,
    /// Its numbers by query (`A`, `C`).
    pub values: Map<String, Value>,
}

/// What a delivery did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AlertsApplied {
    /// Alerts kept, changed or deleted, the heartbeat's included.
    pub applied: usize,
    /// Alerts left out: of a machine unknown or retired, naming none, or
    /// without a `qd_alert` (or a rule's name) the table takes.
    pub unknown: usize,
}

/// Why a delivery was not taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlertsError {
    /// Not an approved or draining `app` machine's certificate.
    Refused,
    /// This API keeps no alerts.
    Unavailable,
    Backend(String),
}

impl MachinesService {
    /// With the machines' active alerts.
    pub fn with_machine_alerts(mut self, alerts: Arc<dyn MachineAlertStore>) -> Self {
        self.alerts = Some(alerts);
        self
    }

    /// Grafana's delivery `alerts`, from the machine of `presented`.
    pub async fn receive_alerts(
        &self,
        presented: &MachineCertificate,
        alerts: &[DeliveredAlert],
        origin: &Origin,
    ) -> Result<AlertsApplied, AlertsError> {
        let store = self.alerts.clone().ok_or(AlertsError::Unavailable)?;
        let caller = self.caller(presented, MachineRole::App).await;
        if caller
            .map_err(|e| AlertsError::Backend(format!("{e:?}")))?
            .is_none()
        {
            let name = presented.common_name.as_deref();
            self.audit_hold(CertificateRefused, name, "alerts:certificate", origin)
                .await;
            return Err(AlertsError::Refused);
        }
        let mut machines = HashMap::new();
        let mut changes = Vec::with_capacity(alerts.len());
        let mut unknown = 0;
        for alert in alerts {
            match self.change(alert, &mut machines).await? {
                Some(change) => changes.push(change),
                None => unknown += 1,
            }
        }
        store
            .apply(&changes, self.clock.now())
            .await
            .map_err(|e| AlertsError::Backend(e.to_string()))?;
        Ok(AlertsApplied {
            applied: changes.len(),
            unknown,
        })
    }

    /// What `alert` changes; `None` when it is left out (an unknown or
    /// retired machine, no `qd_alert`). `machines` keeps the names already
    /// looked up.
    async fn change(
        &self,
        alert: &DeliveredAlert,
        machines: &mut HashMap<String, Option<MachineId>>,
    ) -> Result<Option<AlertChange>, AlertsError> {
        let label = |name: &str| alert.labels.get(name).map(String::as_str);
        let qd_alert = label("qd_alert").unwrap_or_default();
        if qd_alert == ALERTING_HEARTBEAT {
            return Ok(Some(AlertChange::Heartbeat {
                fingerprint: alert.fingerprint.clone(),
                starts_at: alert.starts_at,
            }));
        }
        if !is_name(qd_alert, 64, |c| c.is_ascii_lowercase() || c == '_') {
            tracing::warn!(vm = label("vm"), "an alert with no qd_alert");
            return Ok(None);
        }
        let alertname = label("alertname").unwrap_or_default();
        let key = match label("vm") {
            None if DATASOURCE_ALERTS.contains(&alertname) => {
                let rule_name = label("rulename").unwrap_or_default();
                if !is_name(rule_name, 128, |c| {
                    c.is_ascii_alphanumeric() || "_.-".contains(c)
                }) {
                    tracing::warn!(alertname, qd_alert, "an alert of no rule");
                    return Ok(None);
                }
                AlertKey::Machineless {
                    alertname: alertname.to_string(),
                    rule_name: rule_name.to_string(),
                }
            }
            vm => {
                let name = vm.unwrap_or_default();
                if !machines.contains_key(name) {
                    let found = self.live_machine(name).await?;
                    machines.insert(name.to_string(), found);
                }
                let Some(Some(machine)) = machines.get(name) else {
                    tracing::warn!(vm = name, qd_alert, "an alert of no known machine");
                    return Ok(None);
                };
                AlertKey::Machine {
                    machine: machine.clone(),
                    fingerprint: alert.fingerprint.clone(),
                }
            }
        };
        if !alert.firing {
            return Ok(Some(AlertChange::Resolved(key)));
        }
        Ok(Some(AlertChange::Firing(FiringAlert {
            key,
            fingerprint: alert.fingerprint.clone(),
            qd_alert: qd_alert.to_string(),
            severity: label("severity").and_then(AlertSeverity::parse),
            starts_at: alert.starts_at,
            values: values(alert),
        })))
    }

    /// The machine named `name` that is not retired.
    async fn live_machine(&self, name: &str) -> Result<Option<MachineId>, AlertsError> {
        if name.is_empty() {
            return Ok(None);
        }
        let found = self.registry.find_live(name).await;
        let found = found.map_err(|e| AlertsError::Backend(e.to_string()))?;
        Ok(found.map(|m| m.id))
    }
}

/// `raw` of 1 to `max` characters, each `allowed`: as the table checks it.
fn is_name(raw: &str, max: usize, allowed: impl Fn(char) -> bool) -> bool {
    !raw.is_empty() && raw.len() <= max && raw.chars().all(allowed)
}

/// A row's values: the alert's numbers by query and the labels its text
/// names, nothing else of Grafana's.
fn values(alert: &DeliveredAlert) -> Map<String, Value> {
    let mut values: Map<String, Value> = alert
        .values
        .iter()
        .filter(|(_, v)| v.is_number())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (label, name) in LABELS {
        if let Some(value) = alert.labels.get(label).filter(|v| v.len() <= 128) {
            values.insert(name.to_string(), Value::String(value.clone()));
        }
    }
    values
}
