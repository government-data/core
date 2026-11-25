use super::*;
use std::sync::Mutex;

use async_trait::async_trait;
use auth_gateway::{
    AdminAccount, AuthorizeParams, DirectoryError, DirectoryUser, ExchangeRequest, OAuthError,
    OAuthIdentity,
};
use auth_gateway::{DirectoryAuthenticator, OAuthProvider, ProviderId};
use domain::{AdminId, RepositoryError, Role};

/// Encrypts by prefixing `enc:`, to see what was stored encrypted.
struct TagCipher;
impl SecretCipher for TagCipher {
    fn encrypt(&self, plain: &str) -> Result<String, String> {
        Ok(format!("enc:{plain}"))
    }
    fn decrypt(&self, sealed: &str) -> Result<String, String> {
        sealed
            .strip_prefix("enc:")
            .map(str::to_string)
            .ok_or_else(|| "not encrypted".into())
    }
}

#[derive(Default)]
struct MemStore(Mutex<BTreeMap<String, serde_json::Value>>);
#[async_trait]
impl SettingsStore for MemStore {
    async fn load(&self, section: &str) -> Result<Option<StoredSection>, String> {
        let key = section.to_string();
        Ok(self.0.lock().unwrap().get(&key).map(|d| StoredSection {
            data: d.clone(),
            updated_at: Utc::now(),
            updated_by: Some("a".into()),
        }))
    }
    async fn save(
        &self,
        section: &str,
        data: serde_json::Value,
        updated_by: &str,
    ) -> Result<StoredSection, String> {
        let key = section.to_string();
        self.0.lock().unwrap().insert(key, data.clone());
        Ok(StoredSection {
            data,
            updated_at: Utc::now(),
            updated_by: Some(updated_by.into()),
        })
    }
    async fn delete(&self, section: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(section);
        Ok(())
    }
}

struct NoDirectory;
#[async_trait]
impl DirectoryAuthenticator for NoDirectory {
    async fn authenticate(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Option<DirectoryUser>, DirectoryError> {
        Ok(None)
    }
}

struct NoAccounts;
#[async_trait]
impl DirectoryAccounts for NoAccounts {
    async fn upsert(&self, _: &DirectoryUser, _: &Role) -> Result<AdminAccount, RepositoryError> {
        Err(RepositoryError::Backend("n/a".into()))
    }
    async fn grants_access(&self, _: &[String]) -> Result<bool, RepositoryError> {
        Ok(false)
    }
    async fn sync_groups(&self, _: &AdminId, _: &[String]) -> Result<(), RepositoryError> {
        Ok(())
    }
}

struct StubProvider(ProviderId);
#[async_trait]
impl OAuthProvider for StubProvider {
    fn id(&self) -> ProviderId {
        self.0.clone()
    }
    fn authorize_url(&self, _: &AuthorizeParams) -> String {
        String::new()
    }
    async fn exchange_code(&self, _: &ExchangeRequest) -> Result<OAuthIdentity, OAuthError> {
        Err(OAuthError::Config("n/a".into()))
    }
}

struct Connectors;
#[async_trait]
impl SettingsConnectors for Connectors {
    fn directory(&self, _: &LdapSettings) -> Result<Arc<dyn DirectoryAuthenticator>, String> {
        Ok(Arc::new(NoDirectory))
    }
    async fn probe_directory(
        &self,
        _: &LdapSettings,
        _: Option<&str>,
    ) -> Result<DirectoryProbe, String> {
        Ok(DirectoryProbe::default())
    }
    fn oauth_provider(&self, s: &OAuthProviderSettings) -> Result<Arc<dyn OAuthProvider>, String> {
        Ok(Arc::new(StubProvider(
            ProviderId::parse(&s.id).map_err(|e| e.to_string())?,
        )))
    }
    async fn discover_oidc(&self, _: &str) -> Result<OidcDiscovery, String> {
        Err("n/a".into())
    }
    fn mailer(&self, s: &MailSettings) -> Result<Arc<dyn Mailer>, String> {
        Ok(Arc::new(Named(s.from_address.clone())))
    }
}

/// A mailer that says who sends (in the error, to read it in the test).
struct Named(String);
#[async_trait]
impl Mailer for Named {
    async fn send(&self, _: &OutgoingMail) -> Result<(), String> {
        Err(format!("from {}", self.0))
    }
}

fn service(store: Arc<MemStore>, env: EnvSettings) -> (SettingsService, DirectorySlot) {
    let slot = DirectorySlot::default();
    let oauth = auth_gateway::app::oauth::testing::empty_service();
    let svc = SettingsService::new(
        store,
        Arc::new(TagCipher),
        Arc::new(Connectors),
        SettingsTargets {
            directory: slot.clone(),
            directory_accounts: Arc::new(NoAccounts),
            oauth,
        },
        env,
    );
    (svc, slot)
}

fn ldap() -> LdapSettings {
    LdapSettings {
        enabled: true,
        url: "ldaps://dc.example.com".into(),
        bind_dn: "cn=svc".into(),
        bind_password: "s3cret".into(),
        base_dn: "dc=example,dc=com".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn the_mail_section_sets_how_long_reset_links_work() {
    let store = Arc::new(MemStore::default());
    let (svc, _) = service(store, EnvSettings::default());
    svc.init().await.unwrap();
    let mail = svc.mailer();
    assert_eq!(mail.reset_link_minutes(), None);

    let s = |minutes| MailSettings {
        reset_link_minutes: minutes,
        ..Default::default()
    };
    svc.save_mail(s(45), "admin").await.unwrap();
    assert_eq!(mail.reset_link_minutes(), Some(45));

    for wrong in [MIN_RESET_LINK_MINUTES - 1, MAX_RESET_LINK_MINUTES + 1] {
        let e = svc.save_mail(s(wrong), "admin").await.unwrap_err();
        assert!(
            matches!(&e, SettingsError::Invalid(f) if f.contains_key("reset_link_minutes")),
            "got {e:?}"
        );
    }
    assert_eq!(mail.reset_link_minutes(), Some(45));

    svc.reset("mail", "admin").await.unwrap();
    assert_eq!(mail.reset_link_minutes(), None);
}

#[tokio::test]
async fn the_environment_applies_until_something_is_saved() {
    let store = Arc::new(MemStore::default());
    let (svc, slot) = service(
        store.clone(),
        EnvSettings {
            ldap: Some(ldap()),
            oauth: None,
            reset_link_minutes: 30,
        },
    );
    svc.init().await.unwrap();
    assert!(matches!(svc.ldap().source, SettingsSource::Environment));
    assert!(slot.get().is_some());

    svc.save_ldap(
        LdapSettings {
            enabled: false,
            ..ldap()
        },
        "admin",
    )
    .await
    .unwrap();
    assert!(slot.get().is_none(), "turning it off applies live");
    assert!(matches!(svc.ldap().source, SettingsSource::Database { .. }));
}

#[tokio::test]
async fn secrets_are_stored_encrypted_and_kept_when_sent_empty() {
    let store = Arc::new(MemStore::default());
    let (svc, _) = service(store.clone(), EnvSettings::default());
    svc.init().await.unwrap();
    svc.save_ldap(ldap(), "admin").await.unwrap();
    let raw = store.0.lock().unwrap()["ldap"].clone();
    assert_eq!(raw["bind_password"], "enc:s3cret");

    // Sending it again without a password keeps it.
    let saved = svc
        .save_ldap(
            LdapSettings {
                bind_password: String::new(),
                base_dn: "dc=other".into(),
                ..ldap()
            },
            "admin",
        )
        .await
        .unwrap();
    assert_eq!(saved.value.bind_password, "s3cret");

    // And it survives a restart.
    let (again, _) = service(store, EnvSettings::default());
    again.init().await.unwrap();
    assert_eq!(again.ldap().value.bind_password, "s3cret");
    assert_eq!(again.ldap().value.base_dn, "dc=other");
}

#[tokio::test]
async fn invalid_ldap_names_each_field() {
    let (svc, _) = service(Arc::new(MemStore::default()), EnvSettings::default());
    let err = svc
        .save_ldap(
            LdapSettings {
                enabled: true,
                url: "http://x".into(),
                user_filter: "(mail=x)".into(),
                ..Default::default()
            },
            "admin",
        )
        .await
        .unwrap_err();
    let SettingsError::Invalid(fields) = err else {
        panic!("expected Invalid")
    };
    for f in ["url", "bind_dn", "bind_password", "base_dn", "user_filter"] {
        assert!(fields.contains_key(f), "missing {f}: {fields:?}");
    }
}

#[test]
fn a_provider_saved_before_trust_email_trusts_only_an_own_entra_tenant() {
    let stored = |issuer: &str| -> OAuthProviderSettings {
        serde_json::from_value(serde_json::json!({ "id": "x", "issuer": issuer })).unwrap()
    };
    assert!(stored("https://login.microsoftonline.com/0f1e2d3c/v2.0").trust_email);
    assert!(!stored("https://login.microsoftonline.com/common/v2.0").trust_email);
    assert!(!stored("https://login.microsoftonline.com/organizations/v2.0").trust_email);
    assert!(!stored("https://accounts.google.com").trust_email);
    // What was saved explicitly rules.
    let off: OAuthProviderSettings = serde_json::from_value(serde_json::json!({
        "issuer": "https://login.microsoftonline.com/0f1e2d3c/v2.0",
        "trust_email": false,
    }))
    .unwrap();
    assert!(!off.trust_email);
}

#[tokio::test]
async fn oauth_providers_swap_live_and_keep_secrets_by_id() {
    let (svc, _) = service(Arc::new(MemStore::default()), EnvSettings::default());
    svc.init().await.unwrap();
    let provider = OAuthProviderSettings {
        id: "Entra".into(),
        enabled: true,
        client_id: "cid".into(),
        client_secret: "csecret".into(),
        issuer: "https://login.microsoftonline.com/t/v2.0".into(),
        auth_endpoint: "https://a".into(),
        token_endpoint: "https://t".into(),
        userinfo_endpoint: "https://u".into(),
        ..Default::default()
    };
    svc.save_oauth(
        OAuthSettings {
            providers: vec![provider.clone()],
        },
        "admin",
    )
    .await
    .unwrap();
    assert_eq!(svc.oauth.provider_ids(), vec!["entra".to_string()]);

    let saved = svc
        .save_oauth(
            OAuthSettings {
                providers: vec![OAuthProviderSettings {
                    client_secret: String::new(),
                    ..provider
                }],
            },
            "admin",
        )
        .await
        .unwrap();
    assert_eq!(saved.value.providers[0].client_secret, "csecret");

    svc.save_oauth(OAuthSettings::default(), "admin")
        .await
        .unwrap();
    assert!(svc.oauth.provider_ids().is_empty());
}

#[tokio::test]
async fn the_platform_time_zone_is_utc_until_a_known_one_is_saved() {
    let store = Arc::new(MemStore::default());
    let (svc, _) = service(store.clone(), EnvSettings::default());
    svc.init().await.unwrap();
    assert_eq!(svc.platform().value.timezone, "UTC");
    assert!(matches!(svc.platform().source, SettingsSource::Unset));

    let unknown = PlatformSettings {
        timezone: "Mars/Olympus".into(),
    };
    let Err(SettingsError::Invalid(fields)) = svc.save_platform(unknown, "admin").await else {
        panic!("an unknown zone is refused");
    };
    assert!(fields.contains_key("timezone"));
    assert_eq!(svc.platform().value.timezone, "UTC");

    let madrid = PlatformSettings {
        timezone: " Europe/Madrid ".into(),
    };
    let saved = svc.save_platform(madrid, "admin").await.unwrap();
    assert_eq!(saved.value.timezone, "Europe/Madrid");
    assert_eq!(svc.platform().value.timezone, "Europe/Madrid");

    svc.reset("platform", "admin").await.unwrap();
    assert_eq!(svc.platform().value.timezone, "UTC");
}
