//! The sections of the system settings: what each one holds.

use serde::{Deserialize, Serialize};

/// The LDAP / Active Directory directory for sign-in.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LdapSettings {
    pub enabled: bool,
    /// `ldaps://dc01.company.local:636` or `ldap://…` (with StartTLS).
    pub url: String,
    pub starttls: bool,
    /// Do not check the certificate. Only for tests.
    pub tls_insecure: bool,
    /// The service account users are looked up with.
    pub bind_dn: String,
    pub bind_password: String,
    pub base_dn: String,
    /// A filter with the `{login}` placeholder.
    pub user_filter: String,
    /// The DNs of the groups whose members are global administrators.
    pub admin_groups: Vec<String>,
    pub timeout_secs: u64,
}

/// How mail goes out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailTransport {
    /// Any SMTP server with a user and password.
    #[default]
    Smtp,
    /// Office 365 / Exchange Online through Microsoft Graph (`sendMail`), with
    /// an Entra ID app registration and the `Mail.Send` permission.
    Graph,
}

/// The SMTP connection's encryption.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmtpSecurity {
    /// Unencrypted (port 25, internal networks).
    None,
    /// STARTTLS on the submission port (587).
    #[default]
    Starttls,
    /// Implicit TLS (465).
    Tls,
}

/// Outgoing mail.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MailSettings {
    pub enabled: bool,
    pub transport: MailTransport,
    /// The sender. With Graph, the mailbox it sends from.
    pub from_address: String,
    pub from_name: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: SmtpSecurity,
    /// Empty for a relay without authentication.
    pub smtp_username: String,
    pub smtp_password: String,
    /// The Entra ID tenant's id (GUID) or domain.
    pub graph_tenant_id: String,
    pub graph_client_id: String,
    pub graph_client_secret: String,
    /// How many minutes the password reset link works; `0` leaves
    /// `PASSWORD_RESET_TTL_MINUTES`'s.
    pub reset_link_minutes: u64,
    /// The language of the email notices (`es` / `en`): they go to people who
    /// do not always have an account, so the company sets it. Empty: Spanish.
    pub notice_language: String,
    /// The platform set it (online, the management panel's mail): the server
    /// and its credentials update themselves and the company neither sees nor
    /// changes them; if it saves its own, it stops being the platform's.
    pub platform: bool,
}

/// An OAuth2/OIDC sign-in provider.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, from = "StoredOAuthProvider")]
pub struct OAuthProviderSettings {
    /// It goes in the callback URL (`…/auth/oauth/{id}/callback`) and is the
    /// button's text on the sign-in screen.
    pub id: String,
    pub enabled: bool,
    pub client_id: String,
    pub client_secret: String,
    pub issuer: String,
    pub auth_endpoint: String,
    pub token_endpoint: String,
    pub userinfo_endpoint: String,
    pub scopes: Vec<String>,
    /// Take the email as verified even if the provider sends no
    /// `email_verified` (a single-company Entra ID, for example). Without
    /// this, someone is linked by their email only with `email_verified: true`.
    pub trust_email: bool,
}

/// A provider as it comes stored: older ones have no `trust_email`.
#[derive(Default, Deserialize)]
#[serde(default)]
struct StoredOAuthProvider {
    id: String,
    enabled: bool,
    client_id: String,
    client_secret: String,
    issuer: String,
    auth_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: String,
    scopes: Vec<String>,
    trust_email: Option<bool>,
}

impl From<StoredOAuthProvider> for OAuthProviderSettings {
    fn from(p: StoredOAuthProvider) -> Self {
        Self {
            // Without the field (saved before it existed), what is added today:
            // with a company's own Entra ID tenant the email is trusted, so
            // whoever already signed in by email still does.
            trust_email: p.trust_email.unwrap_or_else(|| entra_own_tenant(&p.issuer)),
            id: p.id,
            enabled: p.enabled,
            client_id: p.client_id,
            client_secret: p.client_secret,
            issuer: p.issuer,
            auth_endpoint: p.auth_endpoint,
            token_endpoint: p.token_endpoint,
            userinfo_endpoint: p.userinfo_endpoint,
            scopes: p.scopes,
        }
    }
}

/// Whether `issuer` is Entra ID with a company's own tenant (not a shared one
/// like `common` or `organizations`): that tenant only hands out its own
/// emails, even without `email_verified`.
pub fn entra_own_tenant(issuer: &str) -> bool {
    let Some(rest) = issuer
        .trim()
        .strip_prefix("https://login.microsoftonline.com/")
    else {
        return false;
    };
    let tenant = rest.split('/').next().unwrap_or("").to_ascii_lowercase();
    !tenant.is_empty() && !["common", "organizations", "consumers"].contains(&tenant.as_str())
}

/// The OAuth2/OIDC providers.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OAuthSettings {
    pub providers: Vec<OAuthProviderSettings>,
}

/// The platform's own settings (decision 0079).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlatformSettings {
    /// The installation's time zone, an IANA name: the update window's and
    /// new schedules' when they name none.
    pub timezone: String,
}

impl Default for PlatformSettings {
    fn default() -> Self {
        Self {
            timezone: schedule_core::PLATFORM_DEFAULT_TIMEZONE.into(),
        }
    }
}

// Secrets are hidden in the logs too.
macro_rules! redacted_debug {
    ($ty:ty, [$($field:ident),*]) => {
        impl std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($ty))
                    $(.field(stringify!($field), &self.$field))*
                    .finish_non_exhaustive()
            }
        }
    };
}
redacted_debug!(LdapSettings, [enabled, url, bind_dn, base_dn, user_filter]);
redacted_debug!(
    MailSettings,
    [enabled, transport, from_address, smtp_host, graph_tenant_id]
);
redacted_debug!(OAuthProviderSettings, [id, enabled, client_id, issuer]);
redacted_debug!(OAuthSettings, [providers]);

/// A section's secret fields, to encrypt them when saving, decrypt them when
/// reading, and keep them when the client sends them empty.
pub(super) trait Secrets {
    fn secrets_mut(&mut self) -> Vec<&mut String>;
}

impl Secrets for LdapSettings {
    fn secrets_mut(&mut self) -> Vec<&mut String> {
        vec![&mut self.bind_password]
    }
}

impl Secrets for MailSettings {
    fn secrets_mut(&mut self) -> Vec<&mut String> {
        vec![&mut self.smtp_password, &mut self.graph_client_secret]
    }
}

impl Secrets for PlatformSettings {
    fn secrets_mut(&mut self) -> Vec<&mut String> {
        Vec::new()
    }
}

impl Secrets for OAuthSettings {
    fn secrets_mut(&mut self) -> Vec<&mut String> {
        self.providers
            .iter_mut()
            .map(|p| &mut p.client_secret)
            .collect()
    }
}

/// The default user filter: an AD user by UPN or email.
pub const DEFAULT_LDAP_USER_FILTER: &str =
    "(&(objectClass=user)(|(userPrincipalName={login})(mail={login})))";
