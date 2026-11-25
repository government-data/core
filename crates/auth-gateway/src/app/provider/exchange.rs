//! Token exchange (RFC 8693, core decision 0089 point 3.2): a module's
//! machine client presents a person's access token (the **subject token**)
//! and gets, for one of its audiences, a token that speaks for that person
//! with the module as the **actor**: 0062's shape, `sub`
//! `person-<account id>`, `act` `{"sub": <module>}`, at most 15 minutes and
//! never past the subject token's `exp`. Only a module's machine client that
//! consumes [`EXCHANGE_CAPABILITY`] may exchange, only a live access token
//! of this provider issued to the same module's people client is taken, and
//! only for an active account. The credential is recorded before it is
//! given; without a record, nothing is given.

use std::time::{Duration, UNIX_EPOCH};

use domain::{AuditEventType, NewActionAudit};
use serde_json::{json, Value};

use super::machine::requested_audience;
use super::{epoch, AuthorizationServer, TokenError, TokenRequest, TokenResponse};
use crate::domain::{AdminAccount, AdminId, ClientAuth, ClientId, OidcClient};

/// The grant type of RFC 8693.
pub const TOKEN_EXCHANGE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
/// The only subject token kind taken today: a person's access token.
pub const ACCESS_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:access_token";
/// The capability a module's machine client must consume to exchange.
pub const EXCHANGE_CAPABILITY: &str = "sql-query";
/// The longest an exchanged token lasts.
const EXCHANGED_TTL: Duration = Duration::from_secs(15 * 60);

impl AuthorizationServer {
    pub(super) async fn exchange(&self, req: &TokenRequest) -> Result<TokenResponse, TokenError> {
        let client = self.requested_client(req).await?;
        match client.auth {
            ClientAuth::Certificate { role } => {
                self.certified_machine(role, req).await?;
            }
            _ => self.check_secret(&client, req)?,
        }
        let module = exchanging_module(&client)?;
        let audience = requested_audience(req, &client)?;
        if req.subject_token_type.as_deref() != Some(ACCESS_TOKEN_TYPE) {
            return Err(TokenError::InvalidRequest);
        }
        let subject = req
            .subject_token
            .as_deref()
            .ok_or(TokenError::InvalidRequest)?;
        let (person, subject_exp) = self.subject_person(subject, &module).await?;
        let now = epoch(self.clock.now());
        let exp = subject_exp.min(now + EXCHANGED_TTL.as_secs());
        let claims = json!({
            "iss": self.issuer,
            "sub": format!("person-{}", person.id.as_str()),
            "aud": audience,
            "act": { "sub": module },
            "iat": now,
            "exp": exp,
            "jti": self.code_tokens.generate(),
        });
        self.record_exchange(&client, &module, &person.id, &audience)
            .await?;
        let signer = self.signers.for_client(&client)?;
        Ok(TokenResponse {
            access_token: signer.sign("at+jwt", &claims.to_string()),
            id_token: None,
            expires_in: exp.saturating_sub(now),
            scope: String::new(),
        })
    }

    /// The active account a live access token of ours speaks for, and the
    /// token's `exp`, if it was issued to `module`'s people client.
    async fn subject_person(
        &self,
        token: &str,
        module: &str,
    ) -> Result<(AdminAccount, u64), TokenError> {
        let claims: Value = self
            .signers
            .verify("at+jwt", token)
            .and_then(|c| serde_json::from_str(&c).ok())
            .ok_or(TokenError::InvalidGrant)?;
        let text = |name: &str| claims.get(name).and_then(Value::as_str);
        let exp = claims.get("exp").and_then(Value::as_u64).unwrap_or(0);
        // A machine token, or an exchanged one, has no person: no `scope`.
        let issued_to_a_person = claims.get("scope").is_some() && claims.get("act").is_none();
        if text("iss") != Some(self.issuer.as_str())
            || exp <= epoch(self.clock.now())
            || !issued_to_a_person
        {
            return Err(TokenError::InvalidGrant);
        }
        let issued_to = text("client_id")
            .and_then(|c| ClientId::parse(c).ok())
            .ok_or(TokenError::InvalidGrant)?;
        let people_client = self
            .clients
            .find(&issued_to)
            .await
            .map_err(|e| TokenError::Internal(e.to_string()))?
            .filter(|c| !c.disabled())
            .ok_or(TokenError::InvalidGrant)?;
        let same_module = people_client
            .settings
            .module
            .as_ref()
            .is_some_and(|m| m.module == module);
        if !same_module {
            return Err(TokenError::InvalidGrant);
        }
        let person = self
            .admins
            .find_by_id(&AdminId::new(text("sub").ok_or(TokenError::InvalidGrant)?))
            .await
            .map_err(|e| TokenError::Internal(e.to_string()))?
            .ok_or(TokenError::InvalidGrant)?;
        let now = self.clock.now();
        let locked = person.lockout.locked_until.is_some_and(|t| t > now);
        if locked || person.password_hash.is_pending_activation() {
            return Err(TokenError::InvalidGrant);
        }
        Ok((person, exp))
    }

    /// Record the credential before it is given (decision 0030 point 3):
    /// the person it speaks for, the module that acts, the audience; never
    /// the token. No record, no token.
    async fn record_exchange(
        &self,
        client: &OidcClient,
        module: &str,
        person: &AdminId,
        audience: &str,
    ) -> Result<(), TokenError> {
        let Some(audit) = &self.audit else {
            return Err(TokenError::Internal(
                "no audit to record the exchange".into(),
            ));
        };
        let event = NewActionAudit {
            event_type: AuditEventType::CredentialsIssued,
            admin_id: Some(person.clone()),
            service_account: Some(client.id.to_string()),
            job_id: None,
            action: format!("token-exchange:{audience}"),
            resource: format!("module/{module}"),
            ip: String::new(),
            user_agent: None,
            occurred_at: UNIX_EPOCH + Duration::from_secs(epoch(self.clock.now())),
        };
        audit
            .record_action(&event)
            .await
            .map(|_| ())
            .map_err(|e| TokenError::Internal(format!("the exchange's record: {e}")))
    }
}

/// The module whose machine client `client` is, if it consumes
/// [`EXCHANGE_CAPABILITY`].
fn exchanging_module(client: &OidcClient) -> Result<String, TokenError> {
    let side = client
        .settings
        .module
        .as_ref()
        .filter(|_| client.machine())
        .ok_or(TokenError::UnauthorizedClient)?;
    if !side.consumes.iter().any(|c| c == EXCHANGE_CAPABILITY) {
        return Err(TokenError::UnauthorizedClient);
    }
    Ok(side.module.clone())
}
