//! Signing in with the email, without typing the company's address (online
//! edition).
//!
//! At `entrar.<domain>` the email and password are typed.
//! [`CentralLoginService::sign_in`] finds the cell's companies with an account
//! for that email and tries the password in each one, inside it (its lockouts,
//! its directory). It only gives the companies where it worked: without the
//! password nobody learns which ones an email belongs to. If it worked in none,
//! the failure is recorded in all of them, as in a normal sign-in.
//!
//! For each company it gives a single-use pass, which its subdomain redeems for
//! the session ([`CentralLoginService::redeem`]): the session cookie belongs to
//! each subdomain, it is not shared.

use std::sync::Arc;
use std::time::Duration;

use crate::{AdminRepository, LoginHandoffStore, ResetTokenGenerator};
use domain::{AdminId, Clock, Email};
use tenancy::{Directory, TenantRecord};

use crate::app::audit::AuditService;
use crate::app::auth::{AuthenticatedAdmin, LoginError, LoginRequest, LoginService};

/// How long a pass works: what the browser takes to reach the subdomain, or the
/// person to pick the company.
pub const HANDOFF_TTL: Duration = Duration::from_secs(5 * 60);

/// The subdomain path that redeems the pass; the token goes in the fragment
/// (`#…`): it reaches no server log.
pub const HANDOFF_PATH: &str = "/handoff";

/// A company one can sign in to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub record: TenantRecord,
    pub admin: AdminId,
    /// `https://<company>.<domain>/handoff#<pass>`.
    pub url: String,
}

#[derive(Clone)]
pub struct CentralLoginService {
    login: LoginService,
    tenants: Arc<dyn Directory>,
    handoffs: Arc<dyn LoginHandoffStore>,
    admins: Arc<dyn AdminRepository>,
    tokens: Arc<dyn ResetTokenGenerator>,
    clock: Arc<dyn Clock>,
    cell: String,
    domain: String,
    audit: Option<AuditService>,
}

impl std::fmt::Debug for CentralLoginService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CentralLoginService")
            .field("cell", &self.cell)
            .field("domain", &self.domain)
            .finish_non_exhaustive()
    }
}

fn internal(e: impl std::fmt::Display) -> LoginError {
    LoginError::Internal(e.to_string())
}

impl CentralLoginService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        login: LoginService,
        tenants: Arc<dyn Directory>,
        handoffs: Arc<dyn LoginHandoffStore>,
        admins: Arc<dyn AdminRepository>,
        tokens: Arc<dyn ResetTokenGenerator>,
        clock: Arc<dyn Clock>,
        cell: String,
        domain: String,
    ) -> Self {
        Self {
            login,
            tenants,
            handoffs,
            admins,
            tokens,
            clock,
            cell,
            domain: domain.trim().trim_matches('.').to_ascii_lowercase(),
            audit: None,
        }
    }

    /// Record in each company's audit trail the failed attempts that belong to
    /// it (the subdomain records the successful ones when it opens the session).
    pub fn with_audit(mut self, audit: AuditService) -> Self {
        self.audit = Some(audit);
        self
    }

    /// The companies where the email and password work, each with its pass.
    /// Never empty: with none it is [`LoginError::InvalidCredentials`] (or
    /// `TooManyAttempts` if one was locked).
    pub async fn sign_in(
        &self,
        request: LoginRequest,
        user_agent: Option<String>,
    ) -> Result<Vec<Entry>, LoginError> {
        let candidates = match Email::parse(&request.email) {
            Ok(email) => self
                .tenants
                .with_email(email.as_str(), &self.cell)
                .await
                .map_err(internal)?,
            Err(_) => Vec::new(),
        };
        if candidates.is_empty() {
            // Outside every company there is no account: the same work (one
            // argon2 verification) and the same answer as a wrong password.
            return Err(self
                .login
                .check(request)
                .await
                .err()
                .unwrap_or(LoginError::InvalidCredentials));
        }

        let mut entries = Vec::new();
        let mut locked = None;
        for record in &candidates {
            let result =
                tenancy::within(record.tenant.clone(), self.login.check(request.clone())).await;
            match result {
                Ok(admin) => entries.push((record.clone(), admin)),
                Err(e @ LoginError::TooManyAttempts { .. }) => locked = Some(e),
                Err(LoginError::InvalidCredentials) => {}
                // One company's directory being down does not take the others down.
                Err(LoginError::DirectoryUnavailable(e)) => {
                    tracing::warn!(tenant = %record.tenant.slug, "central login: directory unavailable: {e}");
                }
                Err(e) => return Err(e),
            }
        }
        if entries.is_empty() {
            let event = |event_type| domain::NewAuditEvent {
                event_type,
                admin_id: None,
                email_attempted: Some(request.email.trim().to_ascii_lowercase()),
                ip: request.client_ip.clone(),
                user_agent: user_agent.clone(),
                occurred_at: self.clock.now(),
            };
            let event_type = if locked.is_some() {
                domain::AuditEventType::LockedOut
            } else {
                domain::AuditEventType::LoginFailed
            };
            for record in &candidates {
                tenancy::within(record.tenant.clone(), async {
                    self.login.record_failure(&request).await?;
                    if let Some(audit) = &self.audit {
                        if let Err(e) = audit.record(event(event_type)).await {
                            tracing::warn!("central login: failed to record audit event: {e}");
                        }
                    }
                    Ok::<_, LoginError>(())
                })
                .await?;
            }
            return Err(locked.unwrap_or(LoginError::InvalidCredentials));
        }

        let mut out = Vec::with_capacity(entries.len());
        for (record, admin) in entries {
            let token = self.tokens.generate();
            let hash = self.tokens.hash(&token);
            let expires_at = self.clock.now() + HANDOFF_TTL;
            tenancy::within(
                record.tenant.clone(),
                self.handoffs.issue(&admin.id, &hash, expires_at),
            )
            .await
            .map_err(internal)?;
            let url = format!(
                "https://{}.{}{HANDOFF_PATH}#{token}",
                record.tenant.slug, self.domain
            );
            out.push(Entry {
                record,
                admin: admin.id,
                url,
            });
        }
        Ok(out)
    }

    /// Redeem a pass in the current company (the subdomain's): the account and
    /// its role to open the session. `None` if it does not work (another
    /// company's, used or expired).
    pub async fn redeem(&self, token: &str) -> Result<Option<AuthenticatedAdmin>, LoginError> {
        let token = token.trim();
        if token.is_empty() {
            return Ok(None);
        }
        let Some(id) = self
            .handoffs
            .redeem(&self.tokens.hash(token), self.clock.now())
            .await
            .map_err(internal)?
        else {
            return Ok(None);
        };
        Ok(self
            .admins
            .find_by_id(&id)
            .await
            .map_err(internal)?
            .map(|account| AuthenticatedAdmin {
                id: account.id,
                role: account.role,
            }))
    }
}
