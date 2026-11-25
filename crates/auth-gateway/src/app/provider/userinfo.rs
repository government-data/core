//! What the provider says about a person besides their tokens: `/userinfo`,
//! the `policy` claim and a module client's role claim (quality-sdk
//! decision 0009).

use serde_json::{json, Map, Value};

use super::{epoch, granted_scopes, identity_claims, AuthorizationServer};
use crate::domain::{AdminAccount, AdminId, ClientId, OidcClient};

impl AuthorizationServer {
    /// `GET /userinfo`: the claims of the person an access token speaks for,
    /// or `None` if the token is not a live one of ours.
    pub async fn userinfo(&self, access_token: &str) -> Option<Value> {
        let claims: Value =
            serde_json::from_str(&self.signers.verify("at+jwt", access_token)?).ok()?;
        if claims.get("iss")?.as_str()? != self.issuer {
            return None;
        }
        if claims.get("exp")?.as_u64()? <= epoch(self.clock.now()) {
            return None;
        }
        // A client deleted or disabled since: its tokens speak for no one.
        let client_id = ClientId::parse(claims.get("client_id")?.as_str()?).ok()?;
        let client = self.clients.find(&client_id).await.ok()??;
        if client.disabled() {
            return None;
        }
        let admin = self
            .admins
            .find_by_id(&AdminId::new(claims.get("sub")?.as_str()?))
            .await
            .ok()??;
        let scopes = granted_scopes(claims.get("scope")?.as_str());
        let mut out = Map::new();
        out.insert("sub".into(), json!(admin.id.as_str()));
        identity_claims(&mut out, &admin, &scopes);
        if let Some(policies) = self.policy_names(&admin).await {
            out.insert("policy".into(), json!(policies));
        }
        if let Some((name, values)) = self.role_claim(&client, &admin).await {
            out.insert(name, json!(values));
        }
        Some(Value::Object(out))
    }

    /// A module client's role claim (quality-sdk decision 0009): its name
    /// and the values whose action `admin` holds now; empty when none, or
    /// when what they hold cannot be read.
    pub(super) async fn role_claim(
        &self,
        client: &OidcClient,
        admin: &AdminAccount,
    ) -> Option<(String, Vec<String>)> {
        let claim = client.settings.module.as_ref()?.role_claim.as_ref()?;
        let held = match &self.actions {
            Some(source) => match source.held(admin, &claim.actions()).await {
                Ok(held) => held,
                Err(e) => {
                    tracing::warn!("role claim: {e}");
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        Some((claim.name.clone(), claim.values_for(&held)))
    }

    /// The account's IAM policy names (ADR 0016), for the `policy` claim, or
    /// `None` without a source. If they cannot be read, an empty list: the
    /// application then grants nothing, rather than signing in without its
    /// permissions known.
    pub(super) async fn policy_names(&self, admin: &AdminAccount) -> Option<Vec<String>> {
        let source = self.policies.as_ref()?;
        Some(match source.policies(admin).await {
            Ok(names) => names,
            Err(e) => {
                tracing::warn!("policy claim: {e}");
                Vec::new()
            }
        })
    }
}

impl AuthorizationServer {
    /// The module a client was registered by (quality-sdk decision 0009);
    /// `None` for a client registered by hand, a disabled one, or none.
    pub async fn client_module(&self, client_id: &str) -> Option<String> {
        let id = ClientId::parse(client_id).ok()?;
        let client = self.clients.find(&id).await.ok()??;
        if client.disabled() {
            return None;
        }
        client.settings.module.map(|m| m.module)
    }

    /// Which of `actions` the account `subject` holds now (quality-sdk
    /// decision 0002, `iam/effective`): `Ok(None)` when there is no such
    /// account; an error when what it holds cannot be read.
    pub async fn held_actions(
        &self,
        subject: &str,
        actions: &[String],
    ) -> Result<Option<Vec<String>>, crate::domain::RepositoryError> {
        let Some(admin) = self.admins.find_by_id(&AdminId::new(subject)).await? else {
            return Ok(None);
        };
        let Some(source) = &self.actions else {
            return Ok(Some(Vec::new()));
        };
        source.held(&admin, actions).await.map(Some)
    }
}
