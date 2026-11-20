//! The license of a local installation: its plan, its features and its
//! limits. The cloud issues it as a **signed permit** (ed25519) and the
//! installation checks it with the public key compiled into it: nobody can
//! forge or edit it to give themselves more.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The plan's caps; `None` is unlimited. Administrators do not count as
/// users, but what they do counts for the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Limits {
    pub servers: Option<u32>,
    pub workflows: Option<u32>,
    /// Users who are not administrators.
    pub users: Option<u32>,
    /// Executions per month: never blocked; a notice when exceeded.
    pub executions_month: Option<u32>,
    pub deployments_month: Option<u32>,
}

impl Limits {
    /// Without a valid license: just enough to keep the basics running.
    pub const BASIC: Self = Self {
        servers: Some(1),
        workflows: Some(5),
        users: Some(2),
        executions_month: Some(500),
        deployments_month: Some(10),
    };
}

/// What the permit the cloud signs says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entitlement {
    pub customer: String,
    pub plan: String,
    /// The features turned on (`python`, `ldap`, `sso`…).
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub limits: Limits,
    /// The installation it was issued for: another cannot use it.
    pub installation: String,
    pub issued_at: DateTime<Utc>,
    /// Until when it is valid (the cloud renews it before).
    pub expires_at: DateTime<Utc>,
    /// Until when the contract is paid (`None`: no end). Not the same as
    /// `expires_at`: the permit is renewed daily even if the contract
    /// expired, so the installation knows where it stands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_ends_at: Option<DateTime<Utc>>,
    /// Suspended by the seller (e.g. a late payment), with the reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspended: Option<String>,
}

impl Entitlement {
    pub fn has(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }
}

/// The installation's billing, as the cloud keeps it: what its
/// administrator sees in License.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Billing {
    /// `mensual`, `semestral` or `anual`; `None` without a contract (a trial).
    pub period: Option<String>,
    /// The add-on packs: code → quantity.
    #[serde(default)]
    pub packages: std::collections::BTreeMap<String, u32>,
    /// What renewing would cost: the last payment, for the same period.
    pub renewal_cop: Option<i64>,
    /// The recorded payments, newest first.
    #[serde(default)]
    pub payments: Vec<BillingPayment>,
}

/// A confirmed payment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingPayment {
    /// The electronic invoice's number.
    pub invoice: String,
    pub paid_on: chrono::NaiveDate,
    /// Colombian pesos, without cents.
    pub amount_cop: i64,
    pub period: String,
    /// The contract period it covers.
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

/// How long is tolerated without talking to the cloud after the permit expired.
pub const GRACE_DAYS: i64 = 7;

/// Days after the end of the contract in which everything goes on, with a notice.
pub const CONTRACT_GRACE_DAYS: i64 = 15;

/// From how many days before the end of the contract the installation warns.
pub const CONTRACT_NOTICE_DAYS: i64 = 60;

/// Where the contract stands. Past the grace period, or suspended, the
/// installation is read-only: everything can be read and scheduled work runs,
/// but nothing is created, changed or deployed. A late payment never stops
/// a cooperative's processes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ContractStatus {
    /// Paid, or without an end.
    Current,
    /// Ends soon: it has to be renewed.
    EndingSoon {
        ends_at: DateTime<Utc>,
        days_left: i64,
    },
    /// Ended recently: everything works until `read_only_from`.
    Grace {
        ended_at: DateTime<Utc>,
        read_only_from: DateTime<Utc>,
    },
    /// Read-only, and why.
    ReadOnly { reason: String },
}

impl ContractStatus {
    pub fn read_only(&self) -> bool {
        matches!(self, Self::ReadOnly { .. })
    }
}

/// The contract status of a permit at a date.
pub fn contract_status(e: &Entitlement, now: DateTime<Utc>) -> ContractStatus {
    if let Some(reason) = &e.suspended {
        return ContractStatus::ReadOnly {
            reason: format!("la licencia está suspendida ({reason})"),
        };
    }
    let Some(ends_at) = e.contract_ends_at else {
        return ContractStatus::Current;
    };
    if now <= ends_at {
        let days_left = (ends_at - now).num_days();
        return if days_left <= CONTRACT_NOTICE_DAYS {
            ContractStatus::EndingSoon { ends_at, days_left }
        } else {
            ContractStatus::Current
        };
    }
    let read_only_from = ends_at + chrono::Duration::days(CONTRACT_GRACE_DAYS);
    if now <= read_only_from {
        ContractStatus::Grace {
            ended_at: ends_at,
            read_only_from,
        }
    } else {
        ContractStatus::ReadOnly {
            reason: format!(
                "el contrato venció el {}: renuévalo para volver a crear, cambiar y desplegar",
                ends_at.format("%Y-%m-%d")
            ),
        }
    }
}

/// Where the installation's license stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LicenseStatus {
    /// A current permit.
    Active,
    /// An expired permit that could not be renewed: still valid until `until`.
    Grace { until: DateTime<Utc> },
    /// No valid permit: the basic plan. `reason` says why.
    Basic { reason: String },
}

/// What to do with a stored permit at a date.
pub fn status_at(entitlement: Option<&Entitlement>, now: DateTime<Utc>) -> LicenseStatus {
    let Some(e) = entitlement else {
        return LicenseStatus::Basic {
            reason: "sin licencia: ingresa la clave de activación".into(),
        };
    };
    if now <= e.expires_at {
        return LicenseStatus::Active;
    }
    let until = e.expires_at + chrono::Duration::days(GRACE_DAYS);
    if now <= until {
        LicenseStatus::Grace { until }
    } else {
        LicenseStatus::Basic {
            reason: format!(
                "la licencia venció el {} y no se pudo renovar con la nube",
                e.expires_at.format("%Y-%m-%d")
            ),
        }
    }
}

/// What is used of the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Usage {
    pub servers: u32,
    pub workflows: u32,
    pub users: u32,
    pub executions_month: u32,
    pub deployments_month: u32,
    /// The seconds the month's finished executions ran: no limit, reported
    /// so the customer sees their usage time.
    pub execution_secs_month: u64,
}

/// What is stored of this installation's license.
#[derive(Debug, Clone, Default)]
pub struct StoredLicense {
    /// The activation key, if it was changed from the UI (otherwise, the
    /// installation's).
    pub key: Option<String>,
    /// The last signed permit the cloud handed out.
    pub permit: Option<String>,
    pub fetched_at: Option<DateTime<Utc>>,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

/// Where the license is stored.
#[async_trait::async_trait]
pub trait LicenseStore: Send + Sync {
    async fn load(&self) -> Result<StoredLicense, String>;
    /// Store another key: the previous permit stops being valid.
    async fn save_key(&self, key: &str) -> Result<(), String>;
    /// A permit just handed out and checked.
    async fn save_permit(&self, permit: &str) -> Result<(), String>;
    /// A failed attempt (the last good permit is kept).
    async fn record_error(&self, error: &str) -> Result<(), String>;
    /// The cloud rejected the key: the stored permit is deleted.
    async fn clear_permit(&self, error: &str) -> Result<(), String>;
    /// The expiry notices already sent (so as not to repeat them).
    async fn reminders_sent(&self) -> Result<Vec<String>, String>;
    async fn mark_reminders(&self, sent: &[String]) -> Result<(), String>;
}

/// Why the permit was not obtained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The cloud was not reached (network, outage): the last permit holds, with grace.
    Unreachable(String),
    /// The cloud said no (key revoked, unknown, or of another installation):
    /// the permit is lost at once.
    Rejected(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(e) | Self::Rejected(e) => f.write_str(e),
        }
    }
}

/// The licensing cloud: it hands out the signed permit and checks it.
#[async_trait::async_trait]
pub trait LicenseAuthority: Send + Sync {
    /// Ask for the current permit of `key` for this installation, reporting
    /// the month's usage (to quote the renewal). Returns the signed permit as
    /// is (it is stored that way and checked again on start-up).
    async fn fetch(
        &self,
        key: &str,
        installation: &str,
        usage: Option<&Usage>,
    ) -> Result<String, FetchError>;
    /// Check a permit's signature and read it.
    fn verify(&self, signed: &str) -> Result<Entitlement, String>;
    /// Tell the cloud the installation is still up, with its administrators'
    /// emails: if it stops, the cloud writes to them (and to nobody else).
    async fn heartbeat(
        &self,
        _key: &str,
        _installation: &str,
        _notify: &[String],
    ) -> Result<(), String> {
        Ok(())
    }
    /// The billing of `key` for this installation.
    async fn billing(&self, _key: &str, _installation: &str) -> Result<Billing, String> {
        Err("la facturación no está disponible".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permit(expires_in_days: i64) -> Entitlement {
        let now = Utc::now();
        Entitlement {
            customer: "Empresa".into(),
            plan: "Profesional".into(),
            features: vec!["ssh".into()],
            limits: Limits::default(),
            installation: "i".into(),
            issued_at: now,
            expires_at: now + chrono::Duration::days(expires_in_days),
            contract_ends_at: None,
            suspended: None,
        }
    }

    #[test]
    fn a_contract_warns_then_has_grace_then_turns_read_only() {
        let now = Utc::now();
        let ends_in = |days: i64| Entitlement {
            contract_ends_at: Some(now + chrono::Duration::days(days)),
            ..permit(30)
        };
        assert_eq!(contract_status(&permit(30), now), ContractStatus::Current);
        assert_eq!(contract_status(&ends_in(200), now), ContractStatus::Current);
        assert!(matches!(
            contract_status(&ends_in(30), now),
            ContractStatus::EndingSoon {
                days_left: 29 | 30,
                ..
            }
        ));
        let grace = contract_status(&ends_in(-3), now);
        assert!(matches!(grace, ContractStatus::Grace { .. }) && !grace.read_only());
        assert!(contract_status(&ends_in(-(CONTRACT_GRACE_DAYS + 1)), now).read_only());
        let suspended = Entitlement {
            suspended: Some("pago pendiente".into()),
            ..ends_in(200)
        };
        assert!(contract_status(&suspended, now).read_only());
    }

    #[test]
    fn permits_without_contract_fields_still_parse() {
        let old = r#"{"customer":"C","plan":"P","installation":"i",
            "issued_at":"2026-01-01T00:00:00Z","expires_at":"2026-02-01T00:00:00Z"}"#;
        let e: Entitlement = serde_json::from_str(old).unwrap();
        assert_eq!((e.contract_ends_at, e.suspended), (None, None));
    }

    #[test]
    fn active_then_grace_then_basic() {
        let now = Utc::now();
        assert_eq!(status_at(Some(&permit(3)), now), LicenseStatus::Active);
        assert!(matches!(
            status_at(Some(&permit(-2)), now),
            LicenseStatus::Grace { .. }
        ));
        assert!(matches!(
            status_at(Some(&permit(-(GRACE_DAYS + 1))), now),
            LicenseStatus::Basic { .. }
        ));
        assert!(matches!(status_at(None, now), LicenseStatus::Basic { .. }));
        assert!(permit(1).has("ssh") && !permit(1).has("sso"));
    }
}
