//! The `authorization_code` grant: a code from `/authorize` redeemed once
//! for an id_token and an access token about the person.

use serde_json::{json, Map, Value};

use super::{
    epoch, identity_claims, meets_policy, AuthorizationServer, TokenError, TokenRequest,
    TokenResponse, TOKEN_TTL,
};
use crate::domain::{AdminAccount, AuthorizationCode, OidcClient};

impl AuthorizationServer {
    pub(super) async fn redeem_code(
        &self,
        req: &TokenRequest,
    ) -> Result<TokenResponse, TokenError> {
        let client = self.authenticate_client(req).await?;
        let code = req
            .code
            .as_deref()
            .filter(|c| !c.is_empty())
            .ok_or(TokenError::InvalidRequest)?;
        let verifier = req.code_verifier.as_deref();
        if verifier.is_none() && !client.pkce_optional() {
            return Err(TokenError::InvalidRequest);
        }

        // Taken before any check: a code that fails one is spent anyway, so it
        // cannot be retried with other values.
        let record = self
            .codes
            .take(&self.code_tokens.hash(code))
            .await
            .map_err(|e| TokenError::Internal(e.to_string()))?
            .ok_or(TokenError::InvalidGrant)?;
        let now = self.clock.now();
        let pkce = match (&record.code_challenge, verifier) {
            // RFC 7636 §4.1: 43 to 128 characters.
            (Some(challenge), Some(v)) => {
                (43..=128).contains(&v.len()) && self.secrets.challenge(v) == *challenge
            }
            // Without PKCE, the code is bound to the client's secret, which
            // `authenticate_client` already checked. A verifier for a code
            // issued without a challenge is a downgrade: refused.
            (None, None) => client.pkce_optional(),
            _ => false,
        };
        let valid = pkce
            && record.client_id == client.id
            && req.redirect_uri.as_deref() == Some(record.redirect_uri.as_str())
            && now < record.expires_at;
        if !valid {
            return Err(TokenError::InvalidGrant);
        }
        let admin = self
            .admins
            .find_by_id(&record.admin_id)
            .await
            .map_err(|e| TokenError::Internal(e.to_string()))?
            .ok_or(TokenError::InvalidGrant)?;
        // The required policy again: it may have been taken away since.
        let policies = self.policy_names(&admin).await;
        if !meets_policy(&client, policies.as_deref()) {
            return Err(TokenError::InvalidGrant);
        }
        let signer = self.signers.for_client(&client)?;
        let policies = policies.map(|p| json!(p));
        let mut id_token = self.id_token_claims(&client, &admin, &record, policies.clone());
        if let Some((name, values)) = self.role_claim(&client, &admin).await {
            id_token.insert(name, json!(values));
        }
        let access = self.person_access_claims(&client, &admin, &record, policies);
        Ok(TokenResponse {
            access_token: signer.sign("at+jwt", &Value::Object(access).to_string()),
            id_token: Some(signer.sign("JWT", &Value::Object(id_token).to_string())),
            expires_in: TOKEN_TTL.as_secs(),
            scope: record.scopes.join(" "),
        })
    }

    fn id_token_claims(
        &self,
        client: &OidcClient,
        admin: &AdminAccount,
        record: &AuthorizationCode,
        policies: Option<Value>,
    ) -> Map<String, Value> {
        let now = self.clock.now();
        let mut claims = Map::new();
        claims.insert("iss".into(), json!(self.issuer));
        claims.insert("sub".into(), json!(admin.id.as_str()));
        claims.insert("aud".into(), json!(client.id.as_str()));
        claims.insert("iat".into(), json!(epoch(now)));
        claims.insert("exp".into(), json!(epoch(now + TOKEN_TTL)));
        claims.insert("auth_time".into(), json!(epoch(record.auth_time)));
        if let Some(nonce) = &record.nonce {
            claims.insert("nonce".into(), json!(nonce));
        }
        identity_claims(&mut claims, admin, &record.scopes);
        if let Some(policies) = policies {
            claims.insert("policy".into(), policies);
        }
        claims
    }

    fn person_access_claims(
        &self,
        client: &OidcClient,
        admin: &AdminAccount,
        record: &AuthorizationCode,
        policies: Option<Value>,
    ) -> Map<String, Value> {
        let now = self.clock.now();
        let mut claims = Map::new();
        claims.insert("iss".into(), json!(self.issuer));
        claims.insert("sub".into(), json!(admin.id.as_str()));
        claims.insert("aud".into(), json!(client.id.as_str()));
        claims.insert("client_id".into(), json!(client.id.as_str()));
        claims.insert("iat".into(), json!(epoch(now)));
        claims.insert("exp".into(), json!(epoch(now + TOKEN_TTL)));
        claims.insert("jti".into(), json!(self.code_tokens.generate()));
        claims.insert("scope".into(), json!(record.scopes.join(" ")));
        claims.insert("role".into(), json!(admin.role.as_str()));
        if let Some(policies) = policies {
            claims.insert("policy".into(), policies);
        }
        claims
    }
}
