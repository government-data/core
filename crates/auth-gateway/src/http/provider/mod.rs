//! HTTP boundary of the gateway as an OIDC provider: discovery, the JWK Set,
//! `/oauth/authorize`, `/oauth/token` and `/oauth/userinfo`. The rules live in
//! [`AuthorizationServer`]; this module speaks the protocol's HTTP.
//!
//! Outside the OpenAPI document on purpose: these are the standard endpoints,
//! described by the discovery document, not part of the frontend's API.

use axum::extract::{Query, RawQuery, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Form, Json, Router};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use serde_json::json;

use self::form::basic_credentials;
use crate::http::client_cert::{ClientCertificateError, ClientCertificates};
use crate::http::session::SESSION_COOKIE;
use crate::{
    AuthenticatedSession, AuthorizationServer, AuthorizeOutcome, AuthorizeRequest,
    MachineCertificate, SessionService, SessionToken, TokenError, TokenRequest,
};

mod form;

#[derive(Clone)]
pub struct ProviderState {
    server: AuthorizationServer,
    sessions: SessionService,
}

impl std::fmt::Debug for ProviderState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderState").finish_non_exhaustive()
    }
}

/// Mount the provider's endpoints; no machine certificate is ever read.
pub fn routes(server: AuthorizationServer, sessions: SessionService) -> Router {
    routes_with_certificates(server, sessions, ClientCertificates::not_trusted())
}

/// Mount the provider's endpoints, `/oauth/token` reading the certificate
/// Traefik's internal entry point passes as `certificates` says (0066,
/// point 6): with it a machine role's client authenticates.
pub fn routes_with_certificates(
    server: AuthorizationServer,
    sessions: SessionService,
    certificates: ClientCertificates,
) -> Router {
    Router::new()
        .route("/.well-known/openid-configuration", get(discovery_handler))
        .route("/oauth/jwks", get(jwks_handler))
        .route("/oauth/authorize", get(authorize_handler))
        .route("/oauth/token", axum::routing::post(token_handler))
        .route(
            "/oauth/userinfo",
            get(userinfo_handler).post(userinfo_handler),
        )
        .with_state(ProviderState { server, sessions })
        .layer(Extension(certificates))
}

async fn discovery_handler(State(state): State<ProviderState>) -> Response {
    cacheable(Json(state.server.discovery()).into_response())
}

async fn jwks_handler(State(state): State<ProviderState>) -> Response {
    let mut response = (
        [(header::CONTENT_TYPE, "application/jwk-set+json")],
        state.server.jwks(),
    )
        .into_response();
    response = cacheable(response);
    response
}

/// The query of `/authorize`, as the application sent it.
#[derive(Debug, Deserialize)]
pub struct AuthorizeQuery {
    response_type: Option<String>,
    client_id: Option<String>,
    redirect_uri: Option<String>,
    scope: Option<String>,
    state: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    nonce: Option<String>,
    prompt: Option<String>,
}

async fn authorize_handler(
    State(state): State<ProviderState>,
    Query(q): Query<AuthorizeQuery>,
    RawQuery(raw): RawQuery,
    jar: CookieJar,
    headers: HeaderMap,
) -> Response {
    let req = AuthorizeRequest {
        response_type: q.response_type,
        client_id: q.client_id,
        redirect_uri: q.redirect_uri,
        scope: q.scope,
        state: q.state,
        code_challenge: q.code_challenge,
        code_challenge_method: q.code_challenge_method,
        nonce: q.nonce,
        prompt: q.prompt,
    };
    let session = current_session(&state.sessions, &jar).await;
    // The session cookie is SameSite=Strict, so it does not come on the
    // redirect from another site. Repeat the request from this one (a page
    // that reloads itself): that request carries it, and only then is the
    // person really without a session.
    let raw = raw.unwrap_or_default();
    if session.is_none() && fetched_cross_site(&headers) {
        return no_store(Html(bounce_page(&raw)).into_response());
    }
    match state.server.authorize(&req, session.as_ref()).await {
        AuthorizeOutcome::Redirect(to) => redirect(&to),
        AuthorizeOutcome::SignIn => redirect(&sign_in_then_back(&headers, &raw)),
        AuthorizeOutcome::Invalid(why) => no_store(
            (
                StatusCode::BAD_REQUEST,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                format!("This sign-in request cannot be completed: {why}."),
            )
                .into_response(),
        ),
    }
}

/// The form of `/token`.
#[derive(Debug, Deserialize)]
pub struct TokenForm {
    grant_type: Option<String>,
    code: Option<String>,
    redirect_uri: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    code_verifier: Option<String>,
    /// `client_credentials`: the audience, as RFC 8707 or as `audience`.
    resource: Option<String>,
    audience: Option<String>,
}

/// `certificate` is the machine's, if the header is trusted and holds one;
/// otherwise nothing, and a machine role's client is refused like any
/// client without its credential.
async fn token_handler(
    State(state): State<ProviderState>,
    headers: HeaderMap,
    certificate: Result<MachineCertificate, ClientCertificateError>,
    Form(form): Form<TokenForm>,
) -> Response {
    let basic = basic_credentials(&headers);
    let used_basic = basic.is_some();
    // RFC 6749 §2.3.1: Basic or the form, not both.
    if used_basic && form.client_secret.is_some() {
        return token_error(&TokenError::InvalidRequest, false);
    }
    let (client_id, client_secret) = match basic {
        Some((id, secret)) => (Some(id), Some(secret)),
        None => (form.client_id, form.client_secret),
    };
    let req = TokenRequest {
        grant_type: form.grant_type,
        code: form.code,
        redirect_uri: form.redirect_uri,
        client_id,
        client_secret,
        code_verifier: form.code_verifier,
        resource: form.resource,
        audience: form.audience,
        client_certificate: certificate.ok(),
    };
    match state.server.token(&req).await {
        Ok(t) => {
            let mut body = json!({
                "access_token": t.access_token,
                "token_type": "Bearer",
                "expires_in": t.expires_in,
            });
            // A machine gets neither an id_token nor scopes.
            if let Some(id_token) = t.id_token {
                body["id_token"] = json!(id_token);
            }
            if !t.scope.is_empty() {
                body["scope"] = json!(t.scope);
            }
            no_store(Json(body).into_response())
        }
        Err(e) => token_error(&e, used_basic),
    }
}

async fn userinfo_handler(State(state): State<ProviderState>, headers: HeaderMap) -> Response {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match token {
        Some(token) => match state.server.userinfo(token.trim()).await {
            Some(claims) => no_store(Json(claims).into_response()),
            None => bearer_error(Some("invalid_token")),
        },
        None => bearer_error(None),
    }
}

async fn current_session(
    sessions: &SessionService,
    jar: &CookieJar,
) -> Option<AuthenticatedSession> {
    let cookie = jar.get(SESSION_COOKIE)?;
    sessions
        .authenticate(&SessionToken::from_raw(cookie.value()))
        .await
        .ok()
}

/// Whether the browser says the request came from another site (Fetch
/// Metadata). Without the header (an old browser) it is taken as not.
fn fetched_cross_site(headers: &HeaderMap) -> bool {
    headers
        .get("sec-fetch-site")
        .is_some_and(|v| v.as_bytes() == b"cross-site")
}

/// A page that loads `/authorize` again with the same query, from this site.
fn bounce_page(raw_query: &str) -> String {
    let target = html_escape(&format!("authorize?{raw_query}"));
    format!(
        "<!doctype html><meta charset=utf-8><meta name=referrer content=no-referrer>\
         <meta http-equiv=refresh content=\"0;url={target}\">\
         <title>…</title><a href=\"{target}\">Continue</a>"
    )
}

/// The sign-in screen, set to come back to this same request. The path is the
/// browser's: behind Traefik, with the prefix it strips (`/api`).
fn sign_in_then_back(headers: &HeaderMap, raw_query: &str) -> String {
    let prefix = headers
        .get("x-forwarded-prefix")
        .and_then(|v| v.to_str().ok())
        .filter(|p| p.starts_with('/') && !p.contains("//"))
        .unwrap_or("")
        .trim_end_matches('/');
    let back = format!("{prefix}/oauth/authorize?{raw_query}");
    format!("/login?next={}", percent_encode(&back))
}

fn token_error(e: &TokenError, used_basic: bool) -> Response {
    if let TokenError::Internal(detail) = e {
        tracing::error!("oidc: token endpoint failed: {detail}");
    }
    let status = match e {
        TokenError::InvalidClient => StatusCode::UNAUTHORIZED,
        TokenError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    };
    let mut response = no_store((status, Json(json!({ "error": e.code() }))).into_response());
    if used_basic && matches!(e, TokenError::InvalidClient) {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"oauth\""),
        );
    }
    response
}

fn bearer_error(error: Option<&str>) -> Response {
    let challenge = match error {
        Some(e) => format!("Bearer error=\"{e}\""),
        None => "Bearer".to_string(),
    };
    let mut response = StatusCode::UNAUTHORIZED.into_response();
    if let Ok(v) = HeaderValue::from_str(&challenge) {
        response.headers_mut().insert(header::WWW_AUTHENTICATE, v);
    }
    no_store(response)
}

fn redirect(to: &str) -> Response {
    match HeaderValue::from_str(to) {
        Ok(location) => {
            no_store((StatusCode::FOUND, [(header::LOCATION, location)]).into_response())
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Tokens and codes must not be cached (RFC 6749 §5.1).
fn no_store(mut response: Response) -> Response {
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

/// Discovery and keys change only with a new key: a few minutes of cache.
fn cacheable(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    response
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_comes_back_to_the_same_request_under_the_stripped_prefix() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-prefix", HeaderValue::from_static("/api"));
        assert_eq!(
            sign_in_then_back(&headers, "client_id=gestion&state=a%20b"),
            "/login?next=%2Fapi%2Foauth%2Fauthorize%3Fclient_id%3Dgestion%26state%3Da%2520b"
                .replace("%2F", "/")
        );
        // A prefix that is not a path is ignored.
        headers.insert("x-forwarded-prefix", HeaderValue::from_static("//evil.com"));
        assert!(sign_in_then_back(&headers, "a=1").starts_with("/login?next=/oauth/authorize"));
    }

    #[test]
    fn the_bounce_page_reloads_the_same_query_escaped() {
        let page = bounce_page("client_id=gestion&state=\"x\"");
        assert!(page.contains("url=authorize?client_id=gestion&amp;state=&quot;x&quot;"));
        assert!(!page.contains("\"x\""));
    }

    #[test]
    fn only_cross_site_requests_bounce() {
        let mut headers = HeaderMap::new();
        assert!(!fetched_cross_site(&headers));
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        assert!(!fetched_cross_site(&headers));
        headers.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
        assert!(fetched_cross_site(&headers));
    }
}
