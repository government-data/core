//! HTTP boundary for OAuth sign-in: the redirect that starts a flow and the
//! callback that finishes it, issuing a session exactly like a password login.
//! Plus the account page's side: a signed-in admin starts a *link* flow, lists
//! the identities linked to them, and removes one.
//!
//! Holds no business logic — [`OAuthLoginService`] owns the flow; this module
//! only speaks HTTP (redirects, cookies). The sign-in routes are public (no
//! session exists yet); the link and identity routes require one.

use crate::OAuthError;
use crate::{AuditService, CompleteOutcome, OAuthLoginService, SessionService};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get};
use axum::{Extension, Json, Router};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use domain::{AdminId, AuditEventType, NewAuditEvent};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::http::auth::ClientIp;
use crate::http::session::{attach_session_cookies, require_session, CurrentSession};
use http_kit::error::ApiError;

/// The cookie that pins a link flow to the browser that started it: it holds
/// the flow's `state`, and the callback finishes a link only when the two
/// match. `SameSite=Lax` (unlike the `Strict` session cookies) because the
/// callback is a cross-site navigation back from the provider — exactly the
/// request a `Strict` cookie is withheld from.
const LINK_COOKIE: &str = "oauth_link";

/// Where a sign-in started with `?next=` goes back to after the callback (a
/// path of this site), across the round trip to the provider. `Lax` for the
/// same reason as [`LINK_COOKIE`].
const NEXT_COOKIE: &str = "oauth_next";

/// `next` if it is a path of this site (`/…`, not `//host` nor `/\host`).
fn local_path(next: &str) -> Option<&str> {
    (next.starts_with('/') && !next.starts_with("//") && !next.starts_with("/\\")).then_some(next)
}

/// Where the callback sends the browser after it finishes.
#[derive(Debug, Clone)]
pub struct OAuthRedirects {
    /// Path to redirect to after a successful sign-in (default `/`).
    pub success: String,
    /// Path to redirect to after a failed sign-in; an `?error=oauth` query is
    /// appended (default `/login`).
    pub failure: String,
    /// The page a link flow returns to («Cuentas vinculadas»), with
    /// `?linked={provider}` or `?error={reason}` (default `/settings/account`).
    pub account: String,
}

impl Default for OAuthRedirects {
    fn default() -> Self {
        Self {
            success: "/".to_string(),
            failure: "/login".to_string(),
            account: "/settings/account".to_string(),
        }
    }
}

/// State shared by the OAuth routes.
#[derive(Clone)]
pub struct OAuthState {
    oauth: OAuthLoginService,
    sessions: SessionService,
    audit: AuditService,
    redirects: OAuthRedirects,
}

impl std::fmt::Debug for OAuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthState").finish_non_exhaustive()
    }
}

/// Mount the OAuth routes.
pub fn routes(
    oauth: OAuthLoginService,
    sessions: SessionService,
    audit: AuditService,
    redirects: OAuthRedirects,
) -> Router {
    let state = OAuthState {
        oauth,
        sessions: sessions.clone(),
        audit,
        redirects,
    };
    let signed_in = Router::new()
        .route("/auth/oauth/{provider}/link", get(link_handler))
        .route("/auth/me/identities", get(identities_handler))
        .route(
            "/auth/me/identities/{provider}/{subject}",
            delete(unlink_handler),
        )
        .with_state(state.clone())
        .layer(axum::middleware::from_fn_with_state(
            sessions,
            require_session,
        ));
    Router::new()
        .route("/auth/oauth/providers", get(providers_handler))
        .route("/auth/oauth/{provider}/start", get(start_handler))
        .route("/auth/oauth/{provider}/callback", get(callback_handler))
        .with_state(state)
        .merge(signed_in)
}

/// The configured OAuth providers, for the login UI to render a button per one.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProvidersOut {
    /// The enabled provider ids (e.g. `["google"]`), sorted. Empty when OAuth
    /// is configured but has no providers; the whole route is absent (`404`)
    /// when OAuth is disabled entirely — the UI treats both as "no providers".
    providers: Vec<String>,
}

/// `GET /auth/oauth/providers`: list the enabled provider ids. Public — the
/// login page calls it before any session exists.
#[utoipa::path(
    get,
    path = "/auth/oauth/providers",
    responses(
        (status = 200, description = "The enabled OAuth provider ids", body = ProvidersOut),
    ),
    tag = "auth",
)]
pub async fn providers_handler(State(state): State<OAuthState>) -> Response {
    Json(ProvidersOut {
        providers: state.oauth.provider_ids(),
    })
    .into_response()
}

/// `GET /auth/oauth/{provider}/start`: redirect the browser to the provider's
/// authorize URL. `404` for an unknown provider; `500` if the flow cannot be
/// started (a storage failure).
///
/// With `?next=<path>` (a path of this site), the callback goes back there
/// instead of the success path: the account signs someone in on the way to
/// an application (`/api/oauth/authorize?…`).
async fn start_handler(
    Path(provider): Path<String>,
    Query(query): Query<StartQuery>,
    jar: CookieJar,
    State(state): State<OAuthState>,
) -> Response {
    if !state.oauth.has_provider(&provider) {
        return ApiError::not_found("Unknown sign-in provider.").into_response();
    }
    let jar = match query.next.as_deref().and_then(local_path) {
        Some(next) => jar.add(
            Cookie::build((NEXT_COOKIE, next.to_string()))
                .http_only(true)
                .secure(true)
                .same_site(SameSite::Lax)
                .path("/")
                .max_age(time::Duration::minutes(10))
                .build(),
        ),
        None => jar.remove(Cookie::build(NEXT_COOKIE).path("/")),
    };
    match state.oauth.begin(&provider).await {
        Ok(outcome) => (jar, Redirect::to(&outcome.authorize_url)).into_response(),
        Err(e) => ApiError::internal(format!("oauth: failed to begin flow for {provider}: {e}"))
            .into_response(),
    }
}

/// `GET /auth/oauth/{provider}/link`: start a flow that links the provider
/// account to the signed-in admin, whatever email it carries. A full browser
/// redirect like `start`; it also sets the cookie that pins the flow to this
/// browser. A `GET` with no CSRF header is fine: the session cookie is
/// `SameSite=Strict`, so another site cannot start it on someone's behalf.
async fn link_handler(
    Path(provider): Path<String>,
    Extension(current): Extension<CurrentSession>,
    jar: CookieJar,
    State(state): State<OAuthState>,
) -> Response {
    if !state.oauth.has_provider(&provider) {
        return ApiError::not_found("Unknown sign-in provider.").into_response();
    }
    match state
        .oauth
        .begin_link(&provider, AdminId::new(&current.admin_id))
        .await
    {
        Ok(outcome) => {
            let cookie = Cookie::build((LINK_COOKIE, outcome.state))
                .http_only(true)
                .secure(true)
                .same_site(SameSite::Lax)
                .path("/")
                .max_age(time::Duration::minutes(10))
                .build();
            (jar.add(cookie), Redirect::to(&outcome.authorize_url)).into_response()
        }
        Err(e) => ApiError::internal(format!("oauth: failed to begin link for {provider}: {e}"))
            .into_response(),
    }
}

/// One external identity linked to the signed-in admin.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IdentityOut {
    /// The provider id (`google`, `microsoft`, ...).
    provider: String,
    /// The provider's subject id — what unlinking names.
    subject: String,
    /// The email the provider reported, if any.
    email: Option<String>,
    /// When it was linked, in Unix epoch seconds.
    linked_at: i64,
}

/// The identities linked to the signed-in admin.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IdentitiesOut {
    identities: Vec<IdentityOut>,
}

/// `GET /auth/me/identities`: the external identities linked to the signed-in
/// admin.
#[utoipa::path(
    get,
    path = "/auth/me/identities",
    responses(
        (status = 200, description = "The linked identities", body = IdentitiesOut),
        (status = 401, description = "No valid session"),
    ),
    tag = "auth",
)]
pub async fn identities_handler(
    Extension(current): Extension<CurrentSession>,
    State(state): State<OAuthState>,
) -> Response {
    match state
        .oauth
        .linked_identities(&AdminId::new(&current.admin_id))
        .await
    {
        Ok(list) => Json(IdentitiesOut {
            identities: list
                .into_iter()
                .map(|i| IdentityOut {
                    provider: i.provider.as_str().to_string(),
                    subject: i.subject,
                    email: i.email,
                    linked_at: i
                        .linked_at
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0),
                })
                .collect(),
        })
        .into_response(),
        Err(e) => ApiError::internal(format!("oauth: identities: {e}")).into_response(),
    }
}

/// `DELETE /auth/me/identities/{provider}/{subject}`: unlink one of the
/// signed-in admin's identities. `404` when they have no such link.
#[utoipa::path(
    delete,
    path = "/auth/me/identities/{provider}/{subject}",
    params(
        ("provider" = String, Path, description = "The provider id"),
        ("subject" = String, Path, description = "The provider's subject id"),
    ),
    responses(
        (status = 204, description = "Unlinked"),
        (status = 401, description = "No valid session"),
        (status = 403, description = "Missing or mismatched CSRF header"),
        (status = 404, description = "No such linked identity"),
    ),
    tag = "auth",
)]
pub async fn unlink_handler(
    Path((provider, subject)): Path<(String, String)>,
    Extension(current): Extension<CurrentSession>,
    ClientIp(client_ip): ClientIp,
    headers: axum::http::HeaderMap,
    State(state): State<OAuthState>,
) -> Response {
    let admin_id = AdminId::new(&current.admin_id);
    match state.oauth.unlink(&admin_id, &provider, &subject).await {
        Ok(true) => {
            state
                .record(AuditEventType::OAuthUnlinked, admin_id, client_ip, &headers)
                .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => ApiError::not_found("No such linked account.").into_response(),
        Err(OAuthError::Config(_)) => {
            ApiError::not_found("No such linked account.").into_response()
        }
        Err(e) => ApiError::internal(format!("oauth: unlink: {e}")).into_response(),
    }
}

/// The `start` query: where to go back to after signing in.
#[derive(Debug, Deserialize)]
struct StartQuery {
    next: Option<String>,
}

/// The callback query: `state` + `code` on success, or `error` if the provider
/// declined. All optional so a malformed callback is handled, not rejected by
/// extraction.
#[derive(Debug, Deserialize)]
struct CallbackQuery {
    state: Option<String>,
    code: Option<String>,
    error: Option<String>,
}

/// `GET /auth/oauth/{provider}/callback`: finish the flow. On a sign-in, issue
/// a session (fresh cookies, exactly like a password login), record the login
/// to the audit trail, and redirect to the success path; on any failure
/// redirect to the failure path with `?error=oauth`. A link flow — the one the
/// browser's link cookie names — goes back to the account page instead, with
/// `?linked={provider}` or `?error={reason}`. The browser is mid-navigation, so
/// a redirect (not a JSON error) is the right shape here.
async fn callback_handler(
    Path(_provider): Path<String>,
    Query(query): Query<CallbackQuery>,
    ClientIp(client_ip): ClientIp,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
    State(state): State<OAuthState>,
) -> Response {
    // The link cookie is single-use: whatever happens next, it goes.
    let bound = jar.get(LINK_COOKIE).map(|c| c.value().to_string());
    // Where a sign-in goes back to: single-use too.
    let next = jar
        .get(NEXT_COOKIE)
        .and_then(|c| local_path(c.value()).map(str::to_string));
    let jar = jar.remove(Cookie::build(NEXT_COOKIE).path("/"));
    let linking = bound.is_some() && bound == query.state;
    let jar = if bound.is_some() {
        jar.remove(Cookie::build(LINK_COOKIE).path("/"))
    } else {
        jar
    };
    let fail = |reason: &str| {
        if linking {
            state.redirect_account(jar.clone(), &format!("error={reason}"))
        } else {
            (jar.clone(), state.redirect_failure()).into_response()
        }
    };

    // The provider declined, or the callback is missing its parameters.
    if query.error.is_some() {
        return fail("oauth");
    }
    let (Some(cb_state), Some(code)) = (query.state.clone(), query.code) else {
        return fail("oauth");
    };

    let authenticated = match state
        .oauth
        .complete(&cb_state, &code, bound.as_deref())
        .await
    {
        Ok(CompleteOutcome::SignedIn(a)) => a,
        Ok(CompleteOutcome::Linked {
            admin_id, provider, ..
        }) => {
            state
                .record(AuditEventType::OAuthLinked, admin_id, client_ip, &headers)
                .await;
            return state.redirect_account(jar.clone(), &format!("linked={provider}"));
        }
        Err(e) => {
            tracing::warn!("oauth: callback failed: {e}");
            return fail(match e {
                OAuthError::LinkedToAnotherAccount => "taken",
                _ => "oauth",
            });
        }
    };

    let issued = match state
        .sessions
        .start(authenticated.id.clone(), authenticated.role)
        .await
    {
        Ok(issued) => issued,
        Err(e) => {
            tracing::error!("oauth: failed to issue session: {e}");
            return fail("oauth");
        }
    };

    // Best-effort audit, same reasoning as the password login handler.
    state
        .record(
            AuditEventType::LoginSucceeded,
            authenticated.id.clone(),
            client_ip,
            &headers,
        )
        .await;

    let jar = attach_session_cookies(jar, &issued);
    let to = next.as_deref().unwrap_or(&state.redirects.success);
    (jar, Redirect::to(to)).into_response()
}

impl OAuthState {
    /// Best-effort audit record for `admin_id`'s request.
    async fn record(
        &self,
        event_type: AuditEventType,
        admin_id: AdminId,
        ip: String,
        headers: &axum::http::HeaderMap,
    ) {
        let user_agent = headers
            .get(axum::http::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        if let Err(e) = self
            .audit
            .record(NewAuditEvent {
                event_type,
                admin_id: Some(admin_id),
                email_attempted: None,
                ip,
                user_agent,
                occurred_at: SystemTime::now(),
            })
            .await
        {
            tracing::warn!("oauth: failed to record audit event: {e}");
        }
    }

    /// Back to the account page with `query` (`linked=…` or `error=…`).
    fn redirect_account(&self, jar: CookieJar, query: &str) -> Response {
        let sep = if self.redirects.account.contains('?') {
            '&'
        } else {
            '?'
        };
        (
            jar,
            Redirect::to(&format!("{}{sep}{query}", self.redirects.account)),
        )
            .into_response()
    }

    /// Redirect to the failure path with an `?error=oauth` marker.
    fn redirect_failure(&self) -> Response {
        let sep = if self.redirects.failure.contains('?') {
            '&'
        } else {
            '?'
        };
        Redirect::to(&format!("{}{sep}error=oauth", self.redirects.failure)).into_response()
    }
}
