//! The outgoing mail section.

use std::collections::BTreeMap;

use async_trait::async_trait;
use auth_gateway::{MailLoader, MailNotice, MailSlot, MailTone};

use super::{
    invalid, keep_secrets, MailSettings, MailTransport, Section, SettingsError, SettingsService,
    SmtpSecurity, MAIL, MAX_RESET_LINK_MINUTES, MIN_RESET_LINK_MINUTES,
};

impl SettingsService {
    /// The language the current company's email notices go out in.
    pub fn notice_language(&self) -> texts::Language {
        texts::Language::parse(&self.mail().value.notice_language).unwrap_or_default()
    }

    /// The mail in use, for whoever needs to send.
    pub fn mailer(&self) -> MailSlot {
        self.mail.clone()
    }

    /// The recovery links' lifetime when the mail does not set another
    /// (`PASSWORD_RESET_TTL_MINUTES`).
    pub fn reset_link_default_minutes(&self) -> u64 {
        self.env.reset_link_minutes
    }

    pub(super) fn apply_mail(&self, s: &MailSettings) -> Result<(), String> {
        self.mail.set_reset_link_minutes(s.reset_link_minutes);
        self.mail
            .set_notice_language(texts::Language::parse(&s.notice_language).unwrap_or_default());
        let mailer = if s.enabled {
            Some(self.connectors.mailer(s)?)
        } else {
            None
        };
        self.mail.set(mailer);
        Ok(())
    }

    fn complete_mail(&self, mut s: MailSettings) -> Result<MailSettings, SettingsError> {
        keep_secrets(&mut s, &self.read().mail.value);
        for field in [
            &mut s.from_address,
            &mut s.from_name,
            &mut s.smtp_host,
            &mut s.smtp_username,
            &mut s.graph_tenant_id,
            &mut s.graph_client_id,
        ] {
            *field = field.trim().to_string();
        }
        s.notice_language = s.notice_language.trim().to_ascii_lowercase();
        let mut errors = BTreeMap::new();
        if !s.notice_language.is_empty() && texts::Language::parse(&s.notice_language).is_none() {
            errors.insert(
                "notice_language".into(),
                texts::tr!("app.settings.noticeLanguage"),
            );
        }
        if !looks_like_email(&s.from_address) {
            errors.insert(
                "from_address".into(),
                texts::tr!("app.settings.invalidEmail"),
            );
        }
        match s.transport {
            MailTransport::Smtp => {
                if s.smtp_host.is_empty() {
                    errors.insert("smtp_host".into(), "Requerido".into());
                }
                if s.smtp_port == 0 {
                    s.smtp_port = match s.smtp_security {
                        SmtpSecurity::None => 25,
                        SmtpSecurity::Starttls => 587,
                        SmtpSecurity::Tls => 465,
                    };
                }
                if !s.smtp_username.is_empty() && s.smtp_password.is_empty() {
                    errors.insert(
                        "smtp_password".into(),
                        texts::tr!("app.settings.requiredWithUser"),
                    );
                }
            }
            MailTransport::Graph => {
                if s.graph_tenant_id.is_empty() {
                    errors.insert("graph_tenant_id".into(), "Requerido".into());
                }
                if s.graph_client_id.is_empty() {
                    errors.insert("graph_client_id".into(), "Requerido".into());
                }
                if s.graph_client_secret.is_empty() {
                    errors.insert("graph_client_secret".into(), "Requerido".into());
                }
            }
        }
        if errors.is_empty() {
            Ok(s)
        } else {
            Err(SettingsError::Invalid(errors))
        }
    }

    pub async fn save_mail(
        &self,
        input: MailSettings,
        actor: &str,
    ) -> Result<Section<MailSettings>, SettingsError> {
        if input.reset_link_minutes != 0
            && !(MIN_RESET_LINK_MINUTES..=MAX_RESET_LINK_MINUTES)
                .contains(&input.reset_link_minutes)
        {
            return Err(invalid(
                "reset_link_minutes",
                &texts::tr!(
                    "app.settings.resetLinkMinutes",
                    MIN_RESET_LINK_MINUTES = MIN_RESET_LINK_MINUTES,
                    MAX_RESET_LINK_MINUTES = MAX_RESET_LINK_MINUTES
                ),
            ));
        }
        let value = if input.enabled {
            let s = self.complete_mail(input)?;
            self.connectors
                .mailer(&s)
                .map_err(|e| invalid("transport", &e))?;
            s
        } else {
            let mut s = input;
            keep_secrets(&mut s, &self.read().mail.value);
            s
        };
        let source = self.persist(MAIL, &value, actor).await?;
        self.apply_mail(&value).map_err(SettingsError::Internal)?;
        let section = Section { value, source };
        self.current.update(|c| c.mail = section.clone());
        Ok(section)
    }

    /// Send a test email with what is in the form.
    pub async fn test_mail(&self, input: MailSettings, to: &str) -> Result<(), SettingsError> {
        let to = to.trim();
        if !looks_like_email(to) {
            return Err(invalid("to", &texts::tr!("app.settings.invalidEmail")));
        }
        let value = self.complete_mail(input)?;
        let mailer = self
            .connectors
            .mailer(&value)
            .map_err(|e| invalid("transport", &e))?;
        mailer
            .send(
                &MailNotice::new(MailTone::Success, texts::tr!("app.settings.mailWorks"))
                    .badge(texts::tr!("app.settings.testLabel"))
                    .prose(texts::tr!("app.settings.testMailBody"))
                    .mail(
                        vec![to.to_string()],
                        texts::tr!("app.settings.testMailSubject"),
                    ),
            )
            .await
            .map_err(SettingsError::Unreachable)
    }
}

fn looks_like_email(s: &str) -> bool {
    domain::Email::parse(s).is_ok()
}

#[async_trait]
impl MailLoader for SettingsService {
    async fn load_mail(&self) -> Result<MailSlot, String> {
        self.ensure_loaded().await.map_err(|e| e.to_string())?;
        Ok(self.mailer())
    }
}
