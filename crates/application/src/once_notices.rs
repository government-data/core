//! Notices the administrators get once per installation (decision 0090,
//! ruling 78), through the settings' mail as the update notices go: the
//! flip to Trino's results. A notice goes out at the API's start until it
//! was sent, or until there is no one to send it to with mail configured
//! (then it counts as sent: it is never sent late).

use async_trait::async_trait;
use auth_gateway::{MailNotice, MailSlot, MailTone, OutgoingMail};

use crate::admins::AdminContacts;

/// The flip's notice: results come from Trino and gold now.
pub const TRINO_RESULTS: &str = "quality.trino_results";

/// Port: which once-notices went out.
#[async_trait]
pub trait OnceNotices: Send + Sync {
    /// Whether `key` went out.
    async fn sent(&self, key: &str) -> Result<bool, String>;
    /// Records that `key` went out; `false` if another process did first.
    async fn mark_sent(&self, key: &str) -> Result<bool, String>;
}

/// The flip's notice to `to`.
pub fn trino_results_mail(to: Vec<String>) -> OutgoingMail {
    let subject = texts::tr!("app.notices.trinoResults.subject");
    MailNotice::new(MailTone::Info, subject.clone())
        .badge(texts::tr!("app.notices.trinoResults.label"))
        .prose(texts::tr!("app.notices.trinoResults.body"))
        .mail(to, subject)
}

/// Sends the flip's notice unless it went out; what fails is retried at
/// the next start.
pub async fn send_trino_results(
    store: &dyn OnceNotices,
    mail: &MailSlot,
    admins: &dyn AdminContacts,
) {
    match store.sent(TRINO_RESULTS).await {
        Ok(false) => {}
        Ok(true) => return,
        Err(e) => {
            tracing::warn!("once notices: {e}");
            return;
        }
    }
    let to = match admins.admin_emails().await {
        Ok(to) => to,
        Err(e) => {
            tracing::warn!("once notices: could not read whom to notify: {e}");
            return;
        }
    };
    if mail.configured() && !to.is_empty() {
        if let Err(e) = mail.send(&trino_results_mail(to)).await {
            tracing::warn!("once notices: the flip's notice was not sent: {e}");
            return;
        }
    }
    match store.mark_sent(TRINO_RESULTS).await {
        Ok(_) => tracing::info!(notice = TRINO_RESULTS, "once notice sent"),
        Err(e) => tracing::warn!("once notices: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flips_notice_says_where_the_results_and_the_dashboard_are() {
        let m = trino_results_mail(vec!["admin@example.org".into()]);
        assert_eq!(m.to, ["admin@example.org"]);
        assert!(!m.subject.is_empty());
    }
}
