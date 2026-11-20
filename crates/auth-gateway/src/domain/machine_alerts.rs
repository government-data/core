//! What the application keeps of the alerts Grafana delivers about machines
//! (decision 0072, points 11 and 12): each active alert once, removed when
//! it resolves, never a history. A machine's alert is keyed by Grafana's
//! fingerprint; one with no machine (a `DatasourceError` or
//! `DatasourceNoData`, or the heartbeat) by Grafana's `alertname` and the
//! rule's name. The use case is `application`'s; the port is here, beside
//! the machines', so that `infrastructure` implements it.

use std::time::SystemTime;

use domain::RepositoryError;
use serde_json::{Map, Value};

use super::machine::MachineId;

/// The heartbeat's `qd_alert`, and its row's `alertname` and rule name.
pub const ALERTING_HEARTBEAT: &str = "alerting_heartbeat";

/// Which row an alert is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlertKey {
    /// A machine's, by Grafana's fingerprint.
    Machine {
        machine: MachineId,
        fingerprint: String,
    },
    /// No machine's: Grafana could not evaluate a rule.
    Machineless {
        alertname: String,
        rule_name: String,
    },
}

/// An alert's `severity` label; the heartbeat has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertSeverity {
    Warning,
    Critical,
}

impl AlertSeverity {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "warning" => Some(Self::Warning),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

/// A firing alert, as its row keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct FiringAlert {
    pub key: AlertKey,
    pub fingerprint: String,
    /// The key of its text (`machines.alerts.<qd_alert>`).
    pub qd_alert: String,
    pub severity: Option<AlertSeverity>,
    pub starts_at: SystemTime,
    /// Its numbers by query and the labels its text names, never prose.
    pub values: Map<String, Value>,
}

/// One change a delivery makes.
#[derive(Debug, Clone, PartialEq)]
pub enum AlertChange {
    /// Kept: its row made, or changed if anything differs; the same alert
    /// again changes nothing, `received_at` included.
    Firing(FiringAlert),
    /// Its row deleted, if there is one.
    Resolved(AlertKey),
    /// Grafana's alerting is alive: the heartbeat's row, its `received_at`
    /// the delivery's time every time.
    Heartbeat {
        fingerprint: String,
        starts_at: SystemTime,
    },
}

/// Port: the machines' active alerts, in the current tenant.
#[async_trait::async_trait]
pub trait MachineAlertStore: Send + Sync {
    /// Applies `changes`, received `at`, in one transaction.
    async fn apply(&self, changes: &[AlertChange], at: SystemTime) -> Result<(), RepositoryError>;
}

#[cfg(test)]
mod tests {
    use super::AlertSeverity;

    #[test]
    fn a_severity_is_warning_or_critical() {
        for s in [AlertSeverity::Warning, AlertSeverity::Critical] {
            assert_eq!(AlertSeverity::parse(s.as_str()), Some(s));
        }
        for other in ["none", "", "Critical", "info"] {
            assert_eq!(AlertSeverity::parse(other), None, "{other}");
        }
    }
}
