//! The LDAP / Active Directory section: validate, save, test and apply it.

use std::collections::BTreeMap;

use auth_gateway::DirectoryLogin;

use super::{
    clean_list, invalid, keep_secrets, DirectoryMember, DirectoryProbe, LdapSettings, Section,
    SettingsError, SettingsService, DEFAULT_LDAP_USER_FILTER, LDAP,
};

impl SettingsService {
    pub(super) fn apply_ldap(&self, s: &LdapSettings) -> Result<(), String> {
        if !s.enabled {
            self.directory.set(None);
            return Ok(());
        }
        let authenticator = self.connectors.directory(s)?;
        self.directory.set(Some(DirectoryLogin {
            authenticator,
            accounts: self.directory_accounts.clone(),
            admin_groups: s.admin_groups.clone(),
        }));
        Ok(())
    }

    /// What was sent, normalized, with the current password if it came empty.
    fn complete_ldap(&self, mut s: LdapSettings) -> Result<LdapSettings, SettingsError> {
        keep_secrets(&mut s, &self.read().ldap.value);
        s.url = s.url.trim().to_string();
        s.bind_dn = s.bind_dn.trim().to_string();
        s.base_dn = s.base_dn.trim().to_string();
        s.user_filter = s.user_filter.trim().to_string();
        if s.user_filter.is_empty() {
            s.user_filter = DEFAULT_LDAP_USER_FILTER.to_string();
        }
        s.admin_groups = clean_list(s.admin_groups);
        if s.timeout_secs == 0 {
            s.timeout_secs = 5;
        }
        let mut errors = BTreeMap::new();
        let lower = s.url.to_ascii_lowercase();
        if !(lower.starts_with("ldap://") || lower.starts_with("ldaps://")) {
            errors.insert("url".into(), texts::tr!("app.settings.ldapScheme"));
        }
        if lower.starts_with("ldaps://") && s.starttls {
            errors.insert(
                "starttls".into(),
                texts::tr!("app.settings.starttlsForLdap"),
            );
        }
        if s.bind_dn.is_empty() {
            errors.insert("bind_dn".into(), "Requerido".into());
        }
        if s.bind_password.is_empty() {
            errors.insert("bind_password".into(), "Requerida".into());
        }
        if s.base_dn.is_empty() {
            errors.insert("base_dn".into(), "Requerido".into());
        }
        if !s.user_filter.contains("{login}") {
            errors.insert("user_filter".into(), "Debe contener {login}".into());
        }
        if s.timeout_secs > 120 {
            errors.insert(
                "timeout_secs".into(),
                texts::tr!("app.settings.between1And120Seconds"),
            );
        }
        if errors.is_empty() {
            Ok(s)
        } else {
            Err(SettingsError::Invalid(errors))
        }
    }

    pub async fn save_ldap(
        &self,
        input: LdapSettings,
        actor: &str,
    ) -> Result<Section<LdapSettings>, SettingsError> {
        let value = if input.enabled {
            self.complete_ldap(input)?
        } else {
            // Turned off it is saved as is: it may be left half filled.
            let mut s = input;
            keep_secrets(&mut s, &self.read().ldap.value);
            s
        };
        if value.enabled {
            self.connectors
                .directory(&value)
                .map_err(|e| invalid("url", &e))?;
        }
        let source = self.persist(LDAP, &value, actor).await?;
        self.apply_ldap(&value).map_err(SettingsError::Internal)?;
        let section = Section { value, source };
        self.current.update(|c| c.ldap = section.clone());
        Ok(section)
    }

    pub async fn test_ldap(
        &self,
        input: LdapSettings,
        login: Option<&str>,
    ) -> Result<DirectoryProbe, SettingsError> {
        let value = self.complete_ldap(input)?;
        let login = login.map(str::trim).filter(|l| !l.is_empty());
        self.connectors
            .probe_directory(&value, login)
            .await
            .map_err(SettingsError::Unreachable)
    }

    /// The people of the `group_dn` group, with the directory configured now.
    /// `Ok(None)` if LDAP sign-in is off.
    pub async fn directory_group_members(
        &self,
        group_dn: &str,
    ) -> Result<Option<Vec<DirectoryMember>>, String> {
        let ldap = self.ldap().value;
        if !ldap.enabled {
            return Ok(None);
        }
        self.connectors
            .directory_group_members(&ldap, group_dn)
            .await
            .map(Some)
    }
}
