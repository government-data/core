//! Outgoing mail: the [`Mailer`] the installation configured (or a fallback)
//! and the [`MailSlot`] that shares it with whoever sends mail — accounts
//! (activation, password reset), and outside this crate the notices and the
//! updates. Settings → Mail fills it.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;

/// Loads the current tenant's mail settings and gives its [`MailSlot`]
/// (Settings implements it). Whoever emails a link calls it first, so as not
/// to say there is no mail when it was just not read yet.
#[async_trait]
pub trait MailLoader: Send + Sync {
    async fn load_mail(&self) -> Result<MailSlot, String>;
}

/// An email to send: the plain text and, if it is formatted (see
/// [`super::MailNotice`]), the same information in HTML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingMail {
    pub to: Vec<String>,
    pub subject: String,
    pub text: String,
    pub html: Option<String>,
}

/// Sends mail.
#[async_trait]
pub trait Mailer: Send + Sync {
    async fn send(&self, mail: &OutgoingMail) -> Result<(), String>;
}

// --- Mail in use -------------------------------------------------------------

/// A tenant's mail: what it sends with, how long a recovery link works (0: the
/// usual) and which language the notices go out in.
#[derive(Clone, Default)]
struct AppliedMail {
    mailer: Option<Arc<dyn Mailer>>,
    reset_link_minutes: u64,
    notice_language: texts::Language,
}

/// The [`Mailer`] in use, shared with whoever sends mail: the one configured
/// in Settings → Mail (in memory, see [`domain::slot::Slot`]) with how long
/// its password reset links work (`0`: the environment's) and, without one,
/// a fixed fallback (tests and whoever does not go through the settings).
#[derive(Clone, Default)]
pub struct MailSlot {
    applied: domain::slot::Slot<AppliedMail>,
    fallback: Arc<RwLock<Option<Arc<dyn Mailer>>>>,
}

impl std::fmt::Debug for MailSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailSlot")
            .field("configured", &self.get().is_some())
            .field("fallback", &self.fallback_configured())
            .finish()
    }
}

impl MailSlot {
    /// A fixed mailer (for whoever does not go through the settings, like
    /// tests).
    pub fn fixed(mailer: Arc<dyn Mailer>) -> Self {
        let slot = Self::default();
        slot.set_fallback(Some(mailer));
        slot
    }

    /// The mailer used when Settings → Mail configures none.
    pub fn set_fallback(&self, mailer: Option<Arc<dyn Mailer>>) {
        *self
            .fallback
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = mailer;
    }

    /// Whether there is a fallback mailer.
    fn fallback_configured(&self) -> bool {
        self.fallback
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    /// The minutes a password reset link works, if the tenant's mail settings
    /// set them.
    pub fn reset_link_minutes(&self) -> Option<u64> {
        self.applied
            .get()
            .map(|m| m.reset_link_minutes)
            .filter(|m| *m > 0)
    }

    pub fn set_reset_link_minutes(&self, minutes: u64) {
        self.applied.update(|m| m.reset_link_minutes = minutes);
    }

    /// The current tenant's notice language (Settings → Mail): the one for
    /// emails that do not go to an account with its own language.
    pub fn notice_language(&self) -> texts::Language {
        self.applied
            .get()
            .map(|m| m.notice_language)
            .unwrap_or_default()
    }

    pub fn set_notice_language(&self, lang: texts::Language) {
        self.applied.update(|m| m.notice_language = lang);
    }

    /// The configured mailer or, without one, the fallback.
    fn get(&self) -> Option<Arc<dyn Mailer>> {
        self.applied.get().and_then(|m| m.mailer).or_else(|| {
            self.fallback
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })
    }

    pub fn set(&self, mailer: Option<Arc<dyn Mailer>>) {
        self.applied.update(|m| m.mailer = mailer);
    }

    /// Whether there is outgoing mail (the configured one or the fallback).
    pub fn configured(&self) -> bool {
        self.get().is_some()
    }

    /// Send with the current tenant's mail; an error if there is none.
    pub async fn send(&self, mail: &OutgoingMail) -> Result<(), String> {
        match self.get() {
            Some(mailer) => mailer.send(mail).await,
            None => Err(texts::tr!("app.settings.mailNotConfigured")),
        }
    }

    /// Send without waiting, with the current tenant's mail; with no mail,
    /// nothing. For notices that must not delay or fail what causes them (and
    /// that, by timing, must not reveal whether an account exists). An error
    /// goes to the log with `what`.
    pub fn send_detached(&self, mail: OutgoingMail, what: &'static str) {
        if !self.configured() {
            return;
        }
        let slot = self.clone();
        tokio::spawn(async move {
            if let Err(e) = slot.send(&mail).await {
                tracing::warn!("{what}: could not send the email: {e}");
            }
        });
    }
}
