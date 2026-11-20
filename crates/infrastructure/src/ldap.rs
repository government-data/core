//! LDAP / Active Directory adapter for the [`DirectoryAuthenticator`] port.
//!
//! The usual search-then-bind flow, one fresh connection per sign-in:
//!
//! 1. bind as the service account (`LDAP_BIND_DN`);
//! 2. search `LDAP_BASE_DN` with `LDAP_USER_FILTER`, where `{login}` is replaced
//!    by the RFC 4515-escaped login, and require exactly one match;
//! 3. bind as that entry's DN with the submitted password — the directory, not
//!    this code, decides whether the password is right;
//! 4. read `mail`, `displayName`/`cn` and `memberOf` from the search result.
//!
//! `memberOf` lists direct groups only. Nested AD groups need the filter or a
//! group-side mapping to name the leaf group.

use std::time::Duration;

use async_trait::async_trait;
use auth_gateway::{DirectoryAuthenticator, DirectoryError, DirectoryUser};
use domain::Email;
use ldap3::adapters::{Adapter, EntriesOnly, PagedResults};
use ldap3::{ldap_escape, LdapConnAsync, LdapConnSettings, Scope, SearchEntry};

/// `invalidCredentials`: wrong password (or a disabled/expired AD account).
const RC_INVALID_CREDENTIALS: u32 = 49;

/// Connection and lookup settings, all from the environment.
#[derive(Clone)]
pub struct LdapConfig {
    /// `ldaps://dc01.empresa.local:636` or `ldap://...` with StartTLS.
    pub url: String,
    /// Upgrade an `ldap://` connection with StartTLS.
    pub starttls: bool,
    /// Skip certificate verification. Development only.
    pub tls_insecure: bool,
    /// The service account used to look users up.
    pub bind_dn: String,
    pub bind_password: String,
    /// Where users live.
    pub base_dn: String,
    /// Search filter with a `{login}` placeholder.
    pub user_filter: String,
    /// Per-operation timeout.
    pub timeout: Duration,
}

impl std::fmt::Debug for LdapConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LdapConfig")
            .field("url", &self.url)
            .field("starttls", &self.starttls)
            .field("tls_insecure", &self.tls_insecure)
            .field("bind_dn", &self.bind_dn)
            .field("base_dn", &self.base_dn)
            .field("user_filter", &self.user_filter)
            .finish_non_exhaustive()
    }
}

/// The default user filter: an Active Directory user by UPN or mail — the
/// same value the person types in the email field of the login form.
pub const DEFAULT_USER_FILTER: &str =
    "(&(objectClass=user)(|(userPrincipalName={login})(mail={login})))";

impl LdapConfig {
    /// Read `LDAP_*` from the environment. `Ok(None)` when `LDAP_URL` is unset
    /// or empty: directory sign-in is then off and only local accounts work
    /// (unless the settings page turns it on). `LDAP_CA_CERT` applies either
    /// way, so a directory configured from the settings page trusts it too.
    ///
    /// The service password comes from `LDAP_BIND_PASSWORD` or, for Docker
    /// secrets, from the file named by `LDAP_BIND_PASSWORD_FILE`.
    pub fn from_env() -> Result<Option<Self>, String> {
        let var = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        // The internal CA that signs the domain controller's certificate.
        // Before looking at `LDAP_URL`: LDAP is also configured from the
        // Settings page, without `LDAP_*`, and needs the same CA. `ldap3`
        // builds its trust store with `rustls-native-certs`, which reads
        // `SSL_CERT_FILE` if it exists; no other TLS client of this process
        // uses that store (reqwest brings its own roots).
        if let Some(ca) = var("LDAP_CA_CERT") {
            std::fs::read(&ca).map_err(|e| format!("LDAP_CA_CERT {ca:?}: {e}"))?;
            std::env::set_var("SSL_CERT_FILE", &ca);
        }
        let Some(url) = var("LDAP_URL") else {
            return Ok(None);
        };
        let required = |name: &str| {
            var(name).ok_or_else(|| format!("{name} is required when LDAP_URL is set"))
        };
        let flag = |name: &str| {
            var(name)
                .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        };
        let bind_password = match var("LDAP_BIND_PASSWORD") {
            Some(p) => p,
            None => {
                let path = required("LDAP_BIND_PASSWORD_FILE")
                    .map_err(|_| "LDAP_BIND_PASSWORD or LDAP_BIND_PASSWORD_FILE is required when LDAP_URL is set".to_string())?;
                std::fs::read_to_string(&path)
                    .map_err(|e| format!("LDAP_BIND_PASSWORD_FILE {path:?}: {e}"))?
                    .trim_end_matches(['\n', '\r'])
                    .to_string()
            }
        };
        let timeout_secs = var("LDAP_TIMEOUT_SECS")
            .map(|v| {
                v.parse::<u64>()
                    .map_err(|_| format!("LDAP_TIMEOUT_SECS {v:?} is not a number"))
            })
            .transpose()?
            .unwrap_or(5);
        Ok(Some(Self {
            url,
            starttls: flag("LDAP_STARTTLS"),
            tls_insecure: flag("LDAP_TLS_INSECURE"),
            bind_dn: required("LDAP_BIND_DN")?,
            bind_password,
            base_dn: required("LDAP_BASE_DN")?,
            user_filter: var("LDAP_USER_FILTER").unwrap_or_else(|| DEFAULT_USER_FILTER.to_string()),
            timeout: Duration::from_secs(timeout_secs.max(1)),
        }))
    }
}

/// Group DNs from `LDAP_ADMIN_GROUPS` (separated by `;`, since DNs contain
/// commas) whose members get the global admin role.
pub fn ldap_admin_groups_from_env() -> Vec<String> {
    std::env::var("LDAP_ADMIN_GROUPS")
        .unwrap_or_default()
        .split(';')
        .map(|g| g.trim().to_string())
        .filter(|g| !g.is_empty())
        .collect()
}

/// The adapter.
#[derive(Debug, Clone)]
pub struct LdapAuthenticator {
    config: LdapConfig,
}

impl LdapAuthenticator {
    /// Build the adapter. Fails if the filter has no `{login}` placeholder,
    /// which would match the same entry for everyone.
    pub fn new(config: LdapConfig) -> Result<Self, String> {
        if !config.user_filter.contains("{login}") {
            return Err("LDAP_USER_FILTER must contain the {login} placeholder".into());
        }
        Ok(Self { config })
    }

    fn unavailable(err: impl std::fmt::Display) -> DirectoryError {
        DirectoryError(err.to_string())
    }

    /// Connect and bind as the service account (step 1 of a sign-in).
    async fn service_bind(&self) -> Result<ldap3::Ldap, DirectoryError> {
        let cfg = &self.config;
        let settings = LdapConnSettings::new()
            .set_conn_timeout(cfg.timeout)
            .set_starttls(cfg.starttls)
            .set_no_tls_verify(cfg.tls_insecure);
        let (conn, mut ldap) = LdapConnAsync::with_settings(settings, &cfg.url)
            .await
            .map_err(|e| Self::unavailable(format!("cannot connect to {}: {e}", cfg.url)))?;
        ldap3::drive!(conn);
        ldap.with_timeout(cfg.timeout);
        ldap.simple_bind(&cfg.bind_dn, &cfg.bind_password)
            .await
            .and_then(|r| r.success())
            .map_err(|e| Self::unavailable(format!("service bind failed: {e}")))?;
        Ok(ldap)
    }

    /// Check the settings from the admin UI: bind as the service account and,
    /// given a `login`, look that person up with the user filter. No user
    /// password is ever checked.
    pub async fn probe(&self, login: Option<&str>) -> Result<LdapProbe, String> {
        let cfg = &self.config;
        let mut ldap = self.service_bind().await.map_err(|e| e.0)?;
        let Some(login) = login.map(str::trim).filter(|l| !l.is_empty()) else {
            // Without a login, at least prove the base DN exists and is readable.
            ldap.with_timeout(cfg.timeout)
                .search(&cfg.base_dn, Scope::Base, "(objectClass=*)", vec!["1.1"])
                .await
                .and_then(|r| r.success())
                .map_err(|e| format!("base DN {:?} is not readable: {e}", cfg.base_dn))?;
            let _ = ldap.unbind().await;
            return Ok(LdapProbe { user: None });
        };
        let filter = cfg.user_filter.replace("{login}", &ldap_escape(login));
        let (entries, _) = ldap
            .with_timeout(cfg.timeout)
            .search(
                &cfg.base_dn,
                Scope::Subtree,
                &filter,
                vec!["mail", "userPrincipalName", "displayName", "cn", "memberOf"],
            )
            .await
            .and_then(|r| r.success())
            .map_err(|e| format!("user search failed: {e}"))?;
        let _ = ldap.unbind().await;
        let entry = match entries.as_slice() {
            [] => return Ok(LdapProbe { user: None }),
            [entry] => SearchEntry::construct(entry.clone()),
            many => {
                return Err(format!(
                    "the user filter matched {} entries; it must match exactly one",
                    many.len()
                ))
            }
        };
        let first = |name: &str| {
            entry
                .attrs
                .get(name)
                .and_then(|v| v.first())
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        Ok(LdapProbe {
            user: Some(LdapProbeUser {
                email: first("mail").or_else(|| first("userPrincipalName")),
                display_name: first("displayName").or_else(|| first("cn")),
                groups: entry.attrs.get("memberOf").cloned().unwrap_or_default(),
                dn: entry.dn,
            }),
        })
    }

    /// The people directly in `group_dn` (by their `memberOf`) that have a
    /// mail or UPN, sorted by email. Paged, so an Active Directory group past
    /// the 1000-entry server limit still comes back whole. Entries without an
    /// address (computers, nested groups) are skipped.
    pub async fn group_members(&self, group_dn: &str) -> Result<Vec<LdapPerson>, String> {
        let cfg = &self.config;
        let mut ldap = self.service_bind().await.map_err(|e| e.0)?;
        let filter = format!(
            "(&(memberOf={})(|(mail=*)(userPrincipalName=*)))",
            ldap_escape(group_dn.trim())
        );
        let adapters: Vec<Box<dyn Adapter<_, _>>> = vec![
            Box::new(EntriesOnly::new()),
            Box::new(PagedResults::new(500)),
        ];
        let mut search = ldap
            .with_timeout(cfg.timeout)
            .streaming_search_with(
                adapters,
                &cfg.base_dn,
                Scope::Subtree,
                &filter,
                vec!["mail", "userPrincipalName", "displayName", "cn"],
            )
            .await
            .map_err(|e| format!("group search failed: {e}"))?;
        let mut people = Vec::new();
        while let Some(entry) = search
            .next()
            .await
            .map_err(|e| format!("group search failed: {e}"))?
        {
            let entry = SearchEntry::construct(entry);
            let first = |name: &str| {
                entry
                    .attrs
                    .get(name)
                    .and_then(|v| v.first())
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
            };
            if let Some(email) = first("mail").or_else(|| first("userPrincipalName")) {
                people.push(LdapPerson {
                    email: email.to_lowercase(),
                    display_name: first("displayName").or_else(|| first("cn")),
                });
            }
        }
        search
            .finish()
            .await
            .success()
            .map_err(|e| format!("group search failed: {e}"))?;
        let _ = ldap.unbind().await;
        people.sort_by(|a, b| a.email.cmp(&b.email));
        people.dedup_by(|a, b| a.email == b.email);
        Ok(people)
    }
}

/// A group member as [`LdapAuthenticator::group_members`] reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LdapPerson {
    /// Lowercased `mail` (or the UPN when there is no mail).
    pub email: String,
    pub display_name: Option<String>,
}

/// What [`LdapAuthenticator::probe`] found.
#[derive(Debug, Clone)]
pub struct LdapProbe {
    /// The person looked up, when a login was given and matched one entry.
    pub user: Option<LdapProbeUser>,
}

/// A directory entry as the probe reports it.
#[derive(Debug, Clone)]
pub struct LdapProbeUser {
    pub dn: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub groups: Vec<String>,
}

#[async_trait]
impl DirectoryAuthenticator for LdapAuthenticator {
    async fn authenticate(
        &self,
        login: &str,
        password: &str,
    ) -> Result<Option<DirectoryUser>, DirectoryError> {
        // An LDAP simple bind with an empty password is an *unauthenticated*
        // bind, which most servers accept. Never let it reach the directory.
        if login.trim().is_empty() || password.is_empty() {
            return Ok(None);
        }
        let cfg = &self.config;
        let mut ldap = self.service_bind().await?;

        // 2. Find the user.
        let filter = cfg
            .user_filter
            .replace("{login}", &ldap_escape(login.trim()));
        let (entries, _) = ldap
            .with_timeout(cfg.timeout)
            .search(
                &cfg.base_dn,
                Scope::Subtree,
                &filter,
                vec![
                    "mail",
                    "userPrincipalName",
                    "sAMAccountName",
                    "uid",
                    "displayName",
                    "cn",
                    "memberOf",
                ],
            )
            .await
            .and_then(|r| r.success())
            .map_err(|e| Self::unavailable(format!("user search failed: {e}")))?;
        // Zero matches: no such user. More than one: an ambiguous filter —
        // refuse rather than guess which person is signing in.
        let [entry] = entries.as_slice() else {
            if entries.len() > 1 {
                tracing::warn!(
                    count = entries.len(),
                    "LDAP user filter matched several entries"
                );
            }
            let _ = ldap.unbind().await;
            return Ok(None);
        };
        let entry = SearchEntry::construct(entry.clone());

        // 3. Prove the password by binding as the user.
        let bind = ldap
            .with_timeout(cfg.timeout)
            .simple_bind(&entry.dn, password)
            .await
            .map_err(Self::unavailable)?;
        let _ = ldap.unbind().await;
        if bind.rc == RC_INVALID_CREDENTIALS {
            return Ok(None);
        }
        bind.success()
            .map_err(|e| Self::unavailable(format!("user bind failed: {e}")))?;

        // 4. Map the entry.
        let first = |name: &str| {
            entry
                .attrs
                .get(name)
                .and_then(|values| values.first())
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let Some(email) = first("mail")
            .or_else(|| first("userPrincipalName"))
            .and_then(|raw| Email::parse(&raw).ok())
        else {
            tracing::warn!(dn = %entry.dn, "LDAP entry has no usable mail or userPrincipalName");
            return Ok(None);
        };
        Ok(Some(DirectoryUser {
            username: first("sAMAccountName")
                .or_else(|| first("uid"))
                .unwrap_or_else(|| login.trim().to_string()),
            email,
            display_name: first("displayName").or_else(|| first("cn")),
            groups: entry.attrs.get("memberOf").cloned().unwrap_or_default(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(filter: &str) -> LdapConfig {
        LdapConfig {
            url: "ldap://127.0.0.1:1".into(),
            starttls: false,
            tls_insecure: false,
            bind_dn: "cn=svc".into(),
            bind_password: "x".into(),
            base_dn: "dc=example,dc=com".into(),
            user_filter: filter.into(),
            timeout: Duration::from_millis(200),
        }
    }

    #[test]
    fn a_filter_without_the_placeholder_is_rejected() {
        assert!(LdapAuthenticator::new(config("(objectClass=user)")).is_err());
        assert!(LdapAuthenticator::new(config("(mail={login})")).is_ok());
    }

    #[test]
    fn the_ca_is_read_even_without_ldap_url() {
        // Without `LDAP_*` (LDAP configured from the page), an unreadable CA
        // is still a start-up error, not something ignored.
        std::env::remove_var("LDAP_URL");
        std::env::set_var("LDAP_CA_CERT", "/nonexistent/ca-interna.pem");
        let result = LdapConfig::from_env();
        std::env::remove_var("LDAP_CA_CERT");
        let err = result.expect_err("an unreadable CA must fail");
        assert!(err.contains("LDAP_CA_CERT"), "{err}");
    }

    #[tokio::test]
    async fn an_empty_password_never_reaches_the_directory() {
        // The URL points nowhere: reaching it would be an Err, not Ok(None).
        let ldap = LdapAuthenticator::new(config("(mail={login})")).unwrap();
        assert_eq!(ldap.authenticate("ana@example.com", "").await, Ok(None));
    }

    #[test]
    fn the_login_is_escaped_into_the_filter() {
        let filter = "(mail={login})".replace("{login}", &ldap_escape("*)(uid=*"));
        assert_eq!(filter, "(mail=\\2a\\29\\28uid=\\2a)");
    }
}
