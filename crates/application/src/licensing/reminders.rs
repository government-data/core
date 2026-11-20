//! The contract's expiry notices by email to the administrators.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use license_model::{contract_status, ContractStatus};

use super::LicenseService;
use crate::settings::{MailNotice, MailSlot, MailTone};

/// Whom to tell about the contract's expiry.
#[async_trait::async_trait]
pub trait AdminContacts: Send + Sync {
    async fn admin_emails(&self) -> Result<Vec<String>, String>;
}

/// The installation's administrators.
pub struct PlatformAdmins(pub Arc<dyn platform::PlatformRepository>);

impl std::fmt::Debug for PlatformAdmins {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PlatformAdmins")
    }
}

#[async_trait::async_trait]
impl AdminContacts for PlatformAdmins {
    async fn admin_emails(&self) -> Result<Vec<String>, String> {
        self.0.admin_emails().await.map_err(|e| e.to_string())
    }
}

/// How many days before the end of the contract an email is sent.
const REMINDER_DAYS: [i64; 3] = [7, 30, 60];

impl LicenseService {
    /// Email the administrators when the contract is about to end, ends and
    /// becomes read-only (with the Settings mail).
    pub fn with_reminders(mut self, mail: MailSlot, admins: Arc<dyn AdminContacts>) -> Self {
        self.reminders = Some((mail, admins));
        self
    }

    /// Send the expiry notice due today, each only once: at 60, 30 and 7
    /// days, on expiry and when it becomes read-only. Returns the notice
    /// sent. Without mail configured, or without administrators with an
    /// email, it does nothing.
    pub async fn send_reminders(&self, now: DateTime<Utc>) -> Result<Option<String>, String> {
        let Some((mail, admins)) = &self.reminders else {
            return Ok(None);
        };
        let Some(entitlement) = self.valid() else {
            return Ok(None);
        };
        let Some(ends_at) = entitlement.contract_ends_at else {
            return Ok(None);
        };
        let (kind, subject, body, tone) = match contract_status(&entitlement, now) {
            ContractStatus::EndingSoon { days_left, .. } => {
                let Some(days) = REMINDER_DAYS.into_iter().find(|d| days_left <= *d) else {
                    return Ok(None);
                };
                (
                    format!("{days}d"),
                    texts::tr!("app.licensing.expiresInDays", days_left = days_left),
                    texts::tr!(
                        "app.licensing.contractExpiresOn",
                        customer = entitlement.customer,
                        plan = entitlement.plan,
                        format = ends_at.format("%d/%m/%Y"),
                        CONTRACT_GRACE_DAYS = license_model::CONTRACT_GRACE_DAYS
                    ),
                    if days_left <= 7 {
                        MailTone::Warning
                    } else {
                        MailTone::Info
                    },
                )
            }
            ContractStatus::Grace { read_only_from, .. } => (
                "expired".into(),
                texts::tr!("app.licensing.expired"),
                texts::tr!(
                    "app.licensing.contractExpired",
                    customer = entitlement.customer,
                    format = ends_at.format("%d/%m/%Y"),
                    format2 = read_only_from.format("%d/%m/%Y")
                ),
                MailTone::Warning,
            ),
            // Suspended: the seller decided it and the cooperative knows.
            ContractStatus::ReadOnly { .. } if entitlement.suspended.is_none() => (
                "read-only".into(),
                texts::tr!("app.licensing.becameReadOnly"),
                texts::tr!(
                    "app.licensing.graceEnded",
                    customer = entitlement.customer,
                    format = ends_at.format("%d/%m/%Y")
                ),
                MailTone::Danger,
            ),
            _ => return Ok(None),
        };
        // One notice per contract: if it is renewed, the end changes and they
        // run again.
        let contract = ends_at.format("%Y-%m-%d").to_string();
        let key = format!("{contract}:{kind}");
        let mut sent = self.store.reminders_sent().await?;
        if sent.contains(&key) {
            return Ok(None);
        }
        let to = admins.admin_emails().await?;
        if to.is_empty() {
            return Ok(None);
        }
        let notice = MailNotice::new(tone, subject.clone())
            .badge(texts::tr!("app.licensing.label"))
            .prose(body)
            .facts([
                (
                    texts::tr!("app.mail.customer"),
                    entitlement.customer.clone(),
                ),
                (texts::tr!("app.mail.plan"), entitlement.plan.clone()),
                (
                    texts::tr!("app.mail.contractEnd"),
                    ends_at.format("%d/%m/%Y").to_string(),
                ),
            ]);
        mail.send(&notice.mail(to, subject)).await?;
        sent.retain(|k| k.starts_with(&contract));
        sent.push(key.clone());
        self.store.mark_reminders(&sent).await?;
        Ok(Some(key))
    }
}
