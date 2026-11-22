//! The emails to an account: the links to activate it or reset its password,
//! and the security notices (the password changed, it was locked by failed
//! attempts, a provider was linked or unlinked, it was deleted). Each one goes
//! out in the account's language (see [`language`]).

use std::time::Duration;

use domain::{AdminId, Email};

use super::mail::{MailSlot, OutgoingMail};
use super::mail_notice::{MailNotice, MailTone};
use crate::domain::{AdminRepository, ProviderId};

/// The language of the emails to `id`: the one the account chose or, if it
/// chose none, the tenant's notice language.
pub(crate) async fn language(
    admins: &dyn AdminRepository,
    mail: &MailSlot,
    id: &AdminId,
) -> texts::Language {
    admins
        .locale(id)
        .await
        .ok()
        .flatten()
        .and_then(|l| texts::Language::parse(&l))
        .unwrap_or_else(|| mail.notice_language())
}

/// "15 minutes", "1 hour", "2 hours" (in the current language).
pub(crate) fn spoken(ttl: Duration) -> String {
    let minutes = ttl.as_secs().div_ceil(60).max(1);
    if minutes.is_multiple_of(60) {
        texts::tr!("app.account.hours", count = minutes / 60)
    } else {
        texts::tr!("app.account.minutes", count = minutes)
    }
}

fn greeting(name: Option<&str>) -> String {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) => texts::tr!("app.account.greetingName", name = name),
        None => texts::tr!("app.account.greeting"),
    }
}

/// A provider's readable name: `google` → Google.
fn provider_name(provider: &ProviderId) -> String {
    match provider.as_str() {
        "google" => "Google".into(),
        "microsoft" | "entra" | "azure" | "azuread" => "Microsoft".into(),
        "orquestador" => texts::tr!("app.account.commonAccount"),
        other => {
            let mut chars = other.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        }
    }
}

fn to(email: &Email) -> Vec<String> {
    vec![email.as_str().to_string()]
}

pub(crate) fn reset_mail(
    email: &Email,
    name: Option<&str>,
    link: &str,
    ttl: Duration,
) -> OutgoingMail {
    MailNotice::new(MailTone::Info, texts::tr!("app.account.reset.title"))
        .badge(texts::tr!("app.account.label"))
        .prose(texts::tr!(
            "app.account.reset.intro",
            greeting = greeting(name),
            email = email
        ))
        .action(texts::tr!("app.account.reset.button"), link)
        .prose(texts::tr!(
            "app.account.reset.expires",
            expiry = spoken(ttl)
        ))
        .prose(texts::tr!("app.account.reset.notYou"))
        .mail(to(email), texts::tr!("app.account.reset.subject"))
}

pub(crate) fn activation_mail(
    email: &Email,
    name: Option<&str>,
    link: &str,
    ttl: Duration,
) -> OutgoingMail {
    MailNotice::new(MailTone::Info, texts::tr!("app.account.activation.title"))
        .badge(texts::tr!("app.account.label"))
        .prose(texts::tr!(
            "app.account.activation.intro",
            greeting = greeting(name),
            email = email
        ))
        .action(texts::tr!("app.account.activation.button"), link)
        .prose(texts::tr!(
            "app.account.activation.expires",
            expiry = spoken(ttl)
        ))
        .footer(texts::tr!("app.account.activation.notExpected"))
        .mail(to(email), texts::tr!("app.account.activation.subject"))
}

pub(crate) fn directory_mail(email: &Email) -> OutgoingMail {
    MailNotice::new(MailTone::Info, texts::tr!("app.account.directory.title"))
        .badge(texts::tr!("app.account.label"))
        .prose(texts::tr!("app.account.directory.body", email = email))
        .mail(to(email), texts::tr!("app.account.reset.subject"))
}

/// The account chose its password with the activation link.
pub(crate) fn welcome_mail(email: &Email, name: Option<&str>) -> OutgoingMail {
    MailNotice::new(MailTone::Success, texts::tr!("app.account.welcome.title"))
        .badge(texts::tr!("app.account.label"))
        .prose(texts::tr!(
            "app.account.welcome.body",
            greeting = greeting(name),
            email = email
        ))
        .footer(texts::tr!("app.account.passwordNotYou"))
        .mail(to(email), texts::tr!("app.account.welcome.subject"))
}

/// The password changed with a reset link.
pub(crate) fn password_changed_mail(email: &Email, name: Option<&str>) -> OutgoingMail {
    MailNotice::new(MailTone::Warning, texts::tr!("app.account.password.title"))
        .badge(texts::tr!("app.account.security"))
        .prose(texts::tr!(
            "app.account.password.body",
            greeting = greeting(name),
            email = email
        ))
        .callout(texts::tr!("app.account.passwordNotYou"))
        .mail(to(email), texts::tr!("app.account.password.subject"))
}

/// The account was locked by failed attempts for `lock`.
pub(crate) fn locked_mail(
    email: &Email,
    name: Option<&str>,
    attempts: u32,
    lock: Duration,
) -> OutgoingMail {
    MailNotice::new(MailTone::Danger, texts::tr!("app.account.locked.title"))
        .badge(texts::tr!("app.account.security"))
        .prose(texts::tr!(
            "app.account.locked.body",
            greeting = greeting(name),
            email = email,
            attempts = attempts,
            expiry = spoken(lock)
        ))
        .callout(texts::tr!("app.account.locked.notYou"))
        .mail(to(email), texts::tr!("app.account.locked.subject"))
}

/// A provider was linked to the account (`external`: the email there).
pub(crate) fn linked_mail(
    email: &Email,
    name: Option<&str>,
    provider: &ProviderId,
    external: Option<&str>,
) -> OutgoingMail {
    let provider = provider_name(provider);
    MailNotice::new(MailTone::Info, texts::tr!("app.account.linked.title"))
        .badge(texts::tr!("app.account.security"))
        .prose(texts::tr!(
            "app.account.linked.body",
            greeting = greeting(name),
            email = email
        ))
        .facts([
            (texts::tr!("app.account.provider"), provider.clone()),
            (
                texts::tr!("app.account.providerAccount"),
                external.unwrap_or_default().to_string(),
            ),
        ])
        .callout(texts::tr!("app.account.linked.notYou"))
        .mail(
            to(email),
            texts::tr!("app.account.linked.subject", provider = provider),
        )
}

/// A provider was removed from the account.
pub(crate) fn unlinked_mail(
    email: &Email,
    name: Option<&str>,
    provider: &ProviderId,
) -> OutgoingMail {
    let provider = provider_name(provider);
    MailNotice::new(MailTone::Info, texts::tr!("app.account.unlinked.title"))
        .badge(texts::tr!("app.account.security"))
        .prose(texts::tr!(
            "app.account.unlinked.body",
            greeting = greeting(name),
            email = email,
            provider = provider
        ))
        .callout(texts::tr!("app.account.unlinked.notYou"))
        .mail(
            to(email),
            texts::tr!("app.account.unlinked.subject", provider = provider),
        )
}

/// An administrator deleted the account.
pub(crate) fn deleted_mail(email: &Email, name: Option<&str>) -> OutgoingMail {
    MailNotice::new(MailTone::Warning, texts::tr!("app.account.deleted.title"))
        .badge(texts::tr!("app.account.label"))
        .prose(texts::tr!(
            "app.account.deleted.body",
            greeting = greeting(name),
            email = email
        ))
        .footer(texts::tr!("app.account.deleted.mistake"))
        .mail(to(email), texts::tr!("app.account.deleted.subject"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn email() -> Email {
        Email::parse("ana@empresa.co").unwrap()
    }

    #[test]
    fn every_account_mail_has_both_parts_and_names_the_account() {
        let google = ProviderId::parse("google").unwrap();
        let minutes = Duration::from_secs(900);
        for mail in [
            reset_mail(&email(), Some("Ana"), "https://o.co/r#t", minutes),
            activation_mail(&email(), None, "https://o.co/a#t", minutes),
            directory_mail(&email()),
            welcome_mail(&email(), None),
            password_changed_mail(&email(), Some("Ana")),
            locked_mail(&email(), None, 5, minutes),
            linked_mail(&email(), None, &google, Some("ana@example.org")),
            unlinked_mail(&email(), None, &google),
            deleted_mail(&email(), None),
        ] {
            assert_eq!(mail.to, vec!["ana@empresa.co".to_string()]);
            assert!(mail.text.contains("ana@empresa.co"), "{}", mail.text);
            let html = mail.html.as_deref().unwrap();
            assert!(html.contains("ana@empresa.co"), "{}", mail.subject);
            assert!(!mail.subject.contains("{{"), "{}", mail.subject);
            assert!(!mail.text.contains("{{"), "{}", mail.text);
        }
    }

    #[test]
    fn the_links_go_in_a_button_and_in_the_text() {
        let mail = reset_mail(
            &email(),
            None,
            "https://o.co/r#t",
            Duration::from_secs(3600),
        );
        assert!(mail.text.contains("https://o.co/r#t"));
        assert!(mail.text.contains("1 hora"), "{}", mail.text);
        assert!(mail.html.unwrap().contains(r#"href="https://o.co/r#t""#));
    }

    #[test]
    fn a_provider_reads_by_its_name() {
        let id = |s: &str| ProviderId::parse(s).unwrap();
        assert_eq!(provider_name(&id("google")), "Google");
        assert_eq!(provider_name(&id("microsoft")), "Microsoft");
        assert_eq!(provider_name(&id("keycloak")), "Keycloak");
    }
}
