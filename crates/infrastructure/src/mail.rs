//! Outgoing mail: SMTP (any server) or Microsoft Graph for Office 365.
//!
//! With Graph, SMTP AUTH is not needed in the tenant: an Entra ID app
//! registration with the `Mail.Send` application permission (consented by an
//! admin) gets a token by *client credentials* and sends with
//! `POST /users/{sender}/sendMail`. It is wise to limit which mailboxes the
//! app can send from with an Exchange *application access policy*.

use std::time::{Duration, Instant};

use lettre::message::header::ContentType;
use lettre::message::{Mailbox, MultiPart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Mutex;

/// The SMTP connection's encryption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpTls {
    None,
    StartTls,
    Implicit,
}

/// An SMTP server.
#[derive(Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub tls: SmtpTls,
    /// `None` for a relay without authentication.
    pub credentials: Option<(String, String)>,
    pub from_address: String,
    pub from_name: String,
}

impl std::fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tls", &self.tls)
            .finish_non_exhaustive()
    }
}

/// An Entra ID app registration with `Mail.Send`.
#[derive(Clone)]
pub struct GraphConfig {
    pub tenant_id: String,
    pub client_id: String,
    pub client_secret: String,
    /// The mailbox it sends from.
    pub from_address: String,
    pub from_name: String,
}

impl std::fmt::Debug for GraphConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphConfig")
            .field("tenant_id", &self.tenant_id)
            .field("client_id", &self.client_id)
            .field("from_address", &self.from_address)
            .finish_non_exhaustive()
    }
}

const TIMEOUT: Duration = Duration::from_secs(20);

/// Sends over SMTP.
#[derive(Debug, Clone)]
pub struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

fn mailbox(address: &str, name: &str) -> Result<Mailbox, String> {
    let address = address
        .parse()
        .map_err(|e| format!("remitente {address:?}: {e}"))?;
    let name = Some(name.trim().to_string()).filter(|n| !n.is_empty());
    Ok(Mailbox::new(name, address))
}

impl SmtpMailer {
    pub fn new(config: SmtpConfig) -> Result<Self, String> {
        type T = AsyncSmtpTransport<Tokio1Executor>;
        let builder = match config.tls {
            SmtpTls::None => T::builder_dangerous(&config.host),
            SmtpTls::StartTls => T::starttls_relay(&config.host).map_err(|e| e.to_string())?,
            SmtpTls::Implicit => T::relay(&config.host).map_err(|e| e.to_string())?,
        };
        let mut builder = builder.port(config.port).timeout(Some(TIMEOUT));
        if let Some((user, password)) = config.credentials {
            builder = builder.credentials(Credentials::new(user, password));
        }
        Ok(Self {
            transport: builder.build(),
            from: mailbox(&config.from_address, &config.from_name)?,
        })
    }

    /// Sends `text` and, if there is one, `html` as the other version of the
    /// same email (`multipart/alternative`): each client shows the one it can.
    pub async fn send(
        &self,
        to: &[String],
        subject: &str,
        text: &str,
        html: Option<&str>,
    ) -> Result<(), String> {
        let mut message = Message::builder().from(self.from.clone());
        for address in to {
            message = message.to(address
                .parse()
                .map_err(|e| format!("destinatario {address:?}: {e}"))?);
        }
        let message = message.subject(subject);
        let message = match html {
            Some(html) => message.multipart(MultiPart::alternative_plain_html(
                text.to_string(),
                html.to_string(),
            )),
            None => message
                .header(ContentType::TEXT_PLAIN)
                .body(text.to_string()),
        }
        .map_err(|e| e.to_string())?;
        self.transport
            .send(message)
            .await
            .map(|_| ())
            .map_err(|e| format!("SMTP: {e}"))
    }
}

/// Sends through Microsoft Graph.
#[derive(Debug)]
pub struct GraphMailer {
    config: GraphConfig,
    http: reqwest::Client,
    login_base: String,
    graph_base: String,
    token: Mutex<Option<(String, Instant)>>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

#[derive(Deserialize)]
struct TokenError {
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct GraphError {
    error: GraphErrorBody,
}

#[derive(Deserialize)]
struct GraphErrorBody {
    code: String,
    message: String,
}

impl GraphMailer {
    pub fn new(config: GraphConfig) -> Self {
        Self::with_endpoints(
            config,
            "https://login.microsoftonline.com",
            "https://graph.microsoft.com",
        )
    }

    /// With other endpoints: national clouds or a test server.
    pub fn with_endpoints(config: GraphConfig, login_base: &str, graph_base: &str) -> Self {
        Self {
            config,
            http: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .build()
                .expect("building a reqwest client with default TLS cannot fail"),
            login_base: login_base.trim_end_matches('/').to_string(),
            graph_base: graph_base.trim_end_matches('/').to_string(),
            token: Mutex::new(None),
        }
    }

    /// A current token, asking for a new one if it is about to expire.
    async fn token(&self) -> Result<String, String> {
        let mut cached = self.token.lock().await;
        if let Some((token, until)) = cached.as_ref() {
            if Instant::now() < *until {
                return Ok(token.clone());
            }
        }
        let url = format!(
            "{}/{}/oauth2/v2.0/token",
            self.login_base,
            self.config.tenant_id.trim()
        );
        let response = self
            .http
            .post(&url)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.config.client_id.trim()),
                ("client_secret", self.config.client_secret.as_str()),
                ("scope", "https://graph.microsoft.com/.default"),
            ])
            .send()
            .await
            .map_err(|e| format!("Entra ID: {e}"))?;
        let status = response.status();
        let body = response.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            let detail = serde_json::from_str::<TokenError>(&body)
                .ok()
                .and_then(|e| e.error_description.or(e.error))
                .unwrap_or(body);
            // The first line is enough: Entra's description carries trace ids.
            let detail = detail.lines().next().unwrap_or_default().to_string();
            return Err(format!(
                "Entra ID rechazó las credenciales ({status}): {detail}"
            ));
        }
        let token: TokenResponse =
            serde_json::from_str(&body).map_err(|e| format!("respuesta de Entra ID: {e}"))?;
        let until = Instant::now() + Duration::from_secs(token.expires_in.saturating_sub(120));
        *cached = Some((token.access_token.clone(), until));
        Ok(token.access_token)
    }

    /// Graph carries a single body: the HTML if there is one, otherwise the text.
    pub async fn send(
        &self,
        to: &[String],
        subject: &str,
        text: &str,
        html: Option<&str>,
    ) -> Result<(), String> {
        let token = self.token().await?;
        let recipients: Vec<_> = to
            .iter()
            .map(|a| json!({ "emailAddress": { "address": a } }))
            .collect();
        let mut from = json!({ "address": self.config.from_address });
        if !self.config.from_name.trim().is_empty() {
            from["name"] = json!(self.config.from_name.trim());
        }
        let body = json!({
            "message": {
                "subject": subject,
                "body": match html {
                    Some(html) => json!({ "contentType": "HTML", "content": html }),
                    None => json!({ "contentType": "Text", "content": text }),
                },
                "toRecipients": recipients,
                "from": { "emailAddress": from },
            },
            "saveToSentItems": false,
        });
        let url = format!(
            "{}/v1.0/users/{}/sendMail",
            self.graph_base,
            self.config.from_address.trim()
        );
        let response = self
            .http
            .post(&url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Microsoft Graph: {e}"))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let text = response.text().await.unwrap_or_default();
        let detail = serde_json::from_str::<GraphError>(&text)
            .map(|e| format!("{}: {}", e.error.code, e.error.message))
            .unwrap_or(text);
        let hint = match status.as_u16() {
            401 | 403 => {
                " (¿la app tiene el permiso de aplicación Mail.Send con consentimiento de \
                 administrador, y la política de acceso de Exchange le permite este buzón?)"
            }
            404 => " (¿existe el buzón remitente en este tenant?)",
            _ => "",
        };
        Err(format!(
            "Microsoft Graph respondió {status}: {detail}{hint}"
        ))
    }
}

/// El correo configurado, sea cual sea el transporte.
#[derive(Debug)]
pub enum OutboundMailer {
    Smtp(SmtpMailer),
    Graph(GraphMailer),
}

impl OutboundMailer {
    pub async fn send(
        &self,
        to: &[String],
        subject: &str,
        text: &str,
        html: Option<&str>,
    ) -> Result<(), String> {
        match self {
            Self::Smtp(m) => m.send(to, subject, text, html).await,
            Self::Graph(m) => m.send(to, subject, text, html).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_invalid_sender_is_rejected_up_front() {
        let err = SmtpMailer::new(SmtpConfig {
            host: "localhost".into(),
            port: 25,
            tls: SmtpTls::None,
            credentials: None,
            from_address: "no-es-un-correo".into(),
            from_name: String::new(),
        })
        .unwrap_err();
        assert!(err.contains("remitente"));
    }

    #[test]
    fn the_sender_name_is_optional() {
        assert_eq!(
            mailbox("a@b.com", " ").unwrap().to_string(),
            "a@b.com".to_string()
        );
        assert_eq!(
            mailbox("a@b.com", "Orquestador").unwrap().to_string(),
            "Orquestador <a@b.com>".to_string()
        );
    }
}
