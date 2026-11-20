//! OAuth sign-in use cases: begin a flow (build the authorize URL and stash a
//! pending authorization) and complete it (validate `state`, exchange the
//! code, resolve the external identity to a local admin, and hand the delivery
//! layer the [`AdminId`]/[`Role`] to issue a session for). A signed-in admin
//! can also run the flow to *link* an identity to their own account (whatever
//! email it carries), list their links and remove them.
//!
//! [`OAuthLoginService`] holds no knowledge of HTTP or storage; it wires the
//! domain ports the composition root injects. Crucially it never returns —
//! and its ports never surface — an access token or id_token, so the delivery
//! layer has nothing token-shaped to leak to the frontend.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::{
    AdminRepository, AuthorizeParams, ExchangeRequest, LinkedIdentity, OAuthError, OAuthIdentity,
    OAuthIdentityRepository, OAuthProvider, OAuthSecretGenerator, PendingAuthStore,
    PendingAuthorization, ProviderId,
};
use domain::{AdminId, Clock, Email, Role};

use crate::app::account_mail::{language, linked_mail, unlinked_mail};
use crate::app::mail::MailSlot;

/// What [`OAuthLoginService::begin`] hands back: the URL to redirect the
/// browser to. The `nonce`/verifier are already persisted server-side, so
/// nothing secret needs to travel back through the delivery layer.
#[derive(Debug, Clone)]
pub struct BeginOutcome {
    /// The provider authorize URL to redirect the browser to.
    pub authorize_url: String,
    /// The flow's `state` (also in `authorize_url`). A link flow pins it to
    /// the browser in a cookie, and hands it back to [`OAuthLoginService::complete`].
    pub state: String,
}

/// How a completed flow ended.
#[derive(Debug, Clone)]
pub enum CompleteOutcome {
    /// A sign-in: issue a session for this admin.
    SignedIn(OAuthAuthenticatedAdmin),
    /// A link flow: the identity is now linked to this admin.
    Linked {
        admin_id: AdminId,
        provider: ProviderId,
        email: Option<String>,
    },
}

/// The identity a completed OAuth flow resolves to — the same shape
/// [`crate::app::LoginService`] returns, so the delivery layer issues a session the
/// same way for a password login and an OAuth login.
#[derive(Debug, Clone)]
pub struct OAuthAuthenticatedAdmin {
    /// The local admin the external identity maps to.
    pub id: AdminId,
    /// That admin's role.
    pub role: Role,
}

/// How long a started flow may sit before its pending authorization expires.
const PENDING_TTL: Duration = Duration::from_secs(10 * 60);

type ProviderMap = HashMap<String, Arc<dyn OAuthProvider>>;

/// Application service for OAuth sign-in.
///
/// The provider set is shared by every clone and can be replaced at run time
/// ([`Self::set_providers`]) when an admin edits it in the settings.
#[derive(Clone)]
pub struct OAuthLoginService {
    /// Each tenant's (from the settings), and the environment's for a tenant
    /// that has not applied its own yet.
    providers: tenancy::PerTenant<Arc<ProviderMap>>,
    env_providers: Arc<ProviderMap>,
    pending: Arc<dyn PendingAuthStore>,
    identities: Arc<dyn OAuthIdentityRepository>,
    admins: Arc<dyn AdminRepository>,
    secrets: Arc<dyn OAuthSecretGenerator>,
    clock: Arc<dyn Clock>,
    redirect_base: String,
    /// To notify the account when a provider is linked or unlinked.
    mail: MailSlot,
}

impl std::fmt::Debug for OAuthLoginService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthLoginService")
            .field("providers", &self.provider_ids())
            .finish_non_exhaustive()
    }
}

impl OAuthLoginService {
    /// Assemble the service from its ports and the registered providers.
    ///
    /// `redirect_base` is the externally reachable origin (e.g.
    /// `https://admin.example.com`); the per-provider callback URL is
    /// `{redirect_base}/auth/oauth/{provider}/callback`.
    pub fn new(
        providers: Vec<Arc<dyn OAuthProvider>>,
        pending: Arc<dyn PendingAuthStore>,
        identities: Arc<dyn OAuthIdentityRepository>,
        admins: Arc<dyn AdminRepository>,
        secrets: Arc<dyn OAuthSecretGenerator>,
        clock: Arc<dyn Clock>,
        redirect_base: impl Into<String>,
    ) -> Self {
        Self {
            providers: tenancy::PerTenant::default(),
            env_providers: Arc::new(Self::index(providers)),
            pending,
            identities,
            admins,
            secrets,
            clock,
            redirect_base: redirect_base.into(),
            mail: MailSlot::default(),
        }
    }

    /// Email the account when a provider is linked or unlinked.
    #[must_use]
    pub fn with_mail(mut self, mail: MailSlot) -> Self {
        self.mail = mail;
        self
    }

    /// Notify `admin_id` that `provider` was linked (`linked`) or unlinked, in
    /// the background.
    fn notify_link(
        &self,
        admin_id: &AdminId,
        provider: &ProviderId,
        external: Option<&str>,
        linked: bool,
    ) {
        if !self.mail.configured() {
            return;
        }
        let (admins, mail) = (self.admins.clone(), self.mail.clone());
        let (id, provider) = (admin_id.clone(), provider.clone());
        let external = external.map(str::to_string);
        tenancy::spawn(async move {
            let Ok(Some(account)) = admins.find_by_id(&id).await else {
                return;
            };
            let lang = language(admins.as_ref(), &mail, &id).await;
            let notice = texts::within(lang, async {
                let name = account.display_name.as_deref();
                if linked {
                    linked_mail(&account.email, name, &provider, external.as_deref())
                } else {
                    unlinked_mail(&account.email, name, &provider)
                }
            })
            .await;
            mail.send_detached(notice, "oauth link");
        });
    }

    fn index(providers: Vec<Arc<dyn OAuthProvider>>) -> ProviderMap {
        providers
            .into_iter()
            .map(|p| (p.id().as_str().to_string(), p))
            .collect()
    }

    /// The current tenant's.
    fn current(&self) -> Arc<ProviderMap> {
        self.providers
            .get()
            .unwrap_or_else(|| self.env_providers.clone())
    }

    /// Replace the provider set. Flows already started with a provider that
    /// is gone fail at the callback, like any unknown provider.
    pub fn set_providers(&self, providers: Vec<Arc<dyn OAuthProvider>>) {
        self.providers.set(Arc::new(Self::index(providers)));
    }

    /// The external origin the per-provider callback URL is built on.
    pub fn redirect_base(&self) -> &str {
        &self.redirect_base
    }

    /// Whether a provider id is configured — lets the delivery layer answer a
    /// `404` for an unknown provider before doing anything else.
    pub fn has_provider(&self, provider: &str) -> bool {
        self.current().contains_key(provider)
    }

    /// The configured provider ids, sorted for a stable order — the login UI
    /// renders one sign-in button per id it gets back.
    pub fn provider_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.current().keys().cloned().collect();
        ids.sort();
        ids
    }

    fn provider(&self, provider: &ProviderId) -> Result<Arc<dyn OAuthProvider>, OAuthError> {
        self.current()
            .get(provider.as_str())
            .cloned()
            .ok_or_else(|| OAuthError::Config(format!("unknown provider {provider}")))
    }

    fn redirect_uri(&self, provider: &ProviderId) -> String {
        format!(
            "{}/auth/oauth/{}/callback",
            self.redirect_base.trim_end_matches('/'),
            provider
        )
    }

    /// Begin a sign-in flow: generate `state`/`nonce`/PKCE, persist the
    /// pending authorization, and return the provider authorize URL.
    pub async fn begin(&self, provider_raw: &str) -> Result<BeginOutcome, OAuthError> {
        self.start(provider_raw, None).await
    }

    /// Begin a flow that links the identity to `admin_id` (the signed-in
    /// admin) instead of signing in.
    pub async fn begin_link(
        &self,
        provider_raw: &str,
        admin_id: AdminId,
    ) -> Result<BeginOutcome, OAuthError> {
        self.start(provider_raw, Some(admin_id)).await
    }

    async fn start(
        &self,
        provider_raw: &str,
        link_admin: Option<AdminId>,
    ) -> Result<BeginOutcome, OAuthError> {
        let provider_id = ProviderId::parse(provider_raw)?;
        let provider = self.provider(&provider_id)?;

        let state = self.secrets.state();
        let nonce = self.secrets.nonce();
        let pkce = self.secrets.pkce();
        let redirect_uri = self.redirect_uri(&provider_id);

        self.pending
            .insert(&PendingAuthorization {
                state: state.clone(),
                provider: provider_id.clone(),
                nonce: nonce.clone(),
                code_verifier: pkce.verifier().to_string(),
                redirect_uri: redirect_uri.clone(),
                created_at: self.clock.now(),
                link_admin,
            })
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?;

        let authorize_url = provider.authorize_url(&AuthorizeParams {
            state: state.clone(),
            nonce,
            code_challenge: pkce.challenge().to_string(),
            redirect_uri,
        });
        Ok(BeginOutcome {
            authorize_url,
            state,
        })
    }

    /// Complete a flow from the callback's `state` and `code`.
    ///
    /// Validates `state` (one-shot consume — a replay finds nothing), exchanges
    /// the code (which validates the id_token `nonce`), and then:
    ///
    /// - **sign-in**: resolves the external identity to a local admin and
    ///   returns it for the delivery layer to issue a session for. A
    ///   first-time identity whose *verified* email matches an existing admin
    ///   is linked then; any other unlinked identity is refused
    ///   ([`OAuthError::NoLinkedAccount`] / [`OAuthError::EmailNotVerified`])
    ///   — OAuth never silently provisions a new administrator.
    /// - **link**: requires `bound_state` (what the browser's link cookie
    ///   holds) to be this `state`, so a link started by one person cannot be
    ///   finished in someone else's browser, then links the identity to the
    ///   admin that started it, unless it already belongs to someone else.
    pub async fn complete(
        &self,
        state: &str,
        code: &str,
        bound_state: Option<&str>,
    ) -> Result<CompleteOutcome, OAuthError> {
        let pending = self
            .pending
            .consume(state)
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?
            .ok_or(OAuthError::InvalidState)?;

        // A pending authorization older than the TTL is treated as invalid,
        // same as an unknown state — it was already removed by `consume`.
        if self
            .clock
            .now()
            .duration_since(pending.created_at)
            .map(|age| age > PENDING_TTL)
            .unwrap_or(true)
        {
            return Err(OAuthError::InvalidState);
        }
        if pending.link_admin.is_some() && bound_state != Some(pending.state.as_str()) {
            return Err(OAuthError::LinkNotBound);
        }

        let provider = self.provider(&pending.provider)?;
        let identity = provider
            .exchange_code(&ExchangeRequest {
                code: code.to_string(),
                code_verifier: pending.code_verifier,
                redirect_uri: pending.redirect_uri,
                expected_nonce: pending.nonce,
            })
            .await?;

        let outcome = match pending.link_admin {
            Some(admin_id) => self.link_to(&identity, admin_id).await,
            None => {
                let admin_id = self.resolve_admin(&identity).await?;
                // The role comes from the account, looked up by id: the
                // identity's email may differ from (or no longer match) it.
                let account = self
                    .admins
                    .find_by_id(&admin_id)
                    .await
                    .map_err(|e| OAuthError::Internal(e.to_string()))?
                    .ok_or(OAuthError::NoLinkedAccount)?;
                Ok(CompleteOutcome::SignedIn(OAuthAuthenticatedAdmin {
                    id: admin_id,
                    role: account.role,
                }))
            }
        }?;
        // The provider's picture becomes the profile picture (the latest one).
        // If it cannot be stored, the sign-in still goes ahead.
        if let Some(picture) = &identity.picture {
            let admin_id = match &outcome {
                CompleteOutcome::SignedIn(a) => &a.id,
                CompleteOutcome::Linked { admin_id, .. } => admin_id,
            };
            if let Err(e) = self.admins.set_avatar(admin_id, Some(picture)).await {
                tracing::warn!("profile picture could not be saved: {e}");
            }
        }
        Ok(outcome)
    }

    /// Map an external identity to a local admin: an existing link wins; else
    /// an admin with the same, provider-verified email is linked now; else
    /// refuse.
    async fn resolve_admin(&self, identity: &OAuthIdentity) -> Result<AdminId, OAuthError> {
        if let Some(admin_id) = self
            .identities
            .find_admin(&identity.provider, &identity.subject)
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?
        {
            return Ok(admin_id);
        }

        let Some(raw_email) = identity.email.as_deref() else {
            return Err(OAuthError::NoLinkedAccount);
        };
        let email = Email::parse(raw_email)
            .map_err(|e| OAuthError::Provider(format!("provider email: {e}")))?;
        let account = self
            .admins
            .find_by_email(&email)
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?
            .ok_or(OAuthError::NoLinkedAccount)?;
        // Matching by an email the provider does not vouch for would let
        // anyone who can register that address there take over the account.
        if !identity.email_verified {
            return Err(OAuthError::EmailNotVerified);
        }

        self.identities
            .link(
                &identity.provider,
                &identity.subject,
                identity.email.as_deref(),
                &account.id,
            )
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?;
        self.notify_link(
            &account.id,
            &identity.provider,
            identity.email.as_deref(),
            true,
        );
        Ok(account.id)
    }

    /// Link `identity` to `admin_id` (a link flow). Re-linking one's own
    /// identity is fine; taking one linked to another admin is not.
    async fn link_to(
        &self,
        identity: &OAuthIdentity,
        admin_id: AdminId,
    ) -> Result<CompleteOutcome, OAuthError> {
        let owner = self
            .identities
            .find_admin(&identity.provider, &identity.subject)
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?;
        if owner.as_ref().is_some_and(|o| *o != admin_id) {
            return Err(OAuthError::LinkedToAnotherAccount);
        }
        self.admins
            .find_by_id(&admin_id)
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?
            .ok_or(OAuthError::NoLinkedAccount)?;
        self.identities
            .link(
                &identity.provider,
                &identity.subject,
                identity.email.as_deref(),
                &admin_id,
            )
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?;
        if owner.is_none() {
            self.notify_link(
                &admin_id,
                &identity.provider,
                identity.email.as_deref(),
                true,
            );
        }
        Ok(CompleteOutcome::Linked {
            admin_id,
            provider: identity.provider.clone(),
            email: identity.email.clone(),
        })
    }

    /// The identities linked to `admin_id`.
    pub async fn linked_identities(
        &self,
        admin_id: &AdminId,
    ) -> Result<Vec<LinkedIdentity>, OAuthError> {
        self.identities
            .list_for_admin(admin_id)
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))
    }

    /// Remove one of `admin_id`'s links; `false` when it had no such link.
    pub async fn unlink(
        &self,
        admin_id: &AdminId,
        provider_raw: &str,
        subject: &str,
    ) -> Result<bool, OAuthError> {
        let provider = ProviderId::parse(provider_raw)?;
        let removed = self
            .identities
            .unlink(admin_id, &provider, subject)
            .await
            .map_err(|e| OAuthError::Internal(e.to_string()))?;
        if removed {
            self.notify_link(admin_id, &provider, None, false);
        }
        Ok(removed)
    }
}

/// In-memory doubles of the OAuth ports, for tests here and in other crates
/// (the `testing` feature).
#[cfg(any(test, feature = "testing"))]
pub mod testing;

#[cfg(test)]
mod tests;
