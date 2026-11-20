//! The gate Traefik asks before serving an application that does not speak
//! OAuth (forwardAuth): `GET /auth/verify` answers `204` with the person's
//! identity in `X-Auth-*` headers, which Traefik hands to the application
//! (`authResponseHeaders`); without a session, a page goes to sign in and
//! comes back, anything else gets `401`. `?role=admin` also requires that
//! role (`403` otherwise).
//!
//! `/auth/docs` is the same gate for the documentation (`/docs`), which
//! comes back only to pages under `/docs/`.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use axum_extra::extract::cookie::CookieJar;
use domain::Role;
use serde::Deserialize;

use crate::http::session::SESSION_COOKIE;
use crate::{AdminRepository, AuthenticatedSession, SessionService, SessionToken};

/// The account's id.
pub const AUTH_USER_HEADER: &str = "x-auth-user";
/// The account's email.
pub const AUTH_EMAIL_HEADER: &str = "x-auth-email";
/// The session's role.
pub const AUTH_ROLE_HEADER: &str = "x-auth-role";
/// The company's subdomain (online edition); empty on-premise.
pub const AUTH_ORG_HEADER: &str = "x-auth-org";

#[derive(Clone)]
pub struct VerifyState {
    sessions: SessionService,
    admins: Arc<dyn AdminRepository>,
}

impl std::fmt::Debug for VerifyState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifyState").finish_non_exhaustive()
    }
}

/// Mount `/auth/verify` and `/auth/docs`. Both go without
/// [`crate::http::session::require_session`], which would answer `401`
/// instead of sending a page to sign in.
pub fn routes(sessions: SessionService, admins: Arc<dyn AdminRepository>) -> Router {
    Router::new()
        .route("/auth/verify", get(verify_handler))
        .route("/auth/docs", get(docs_handler))
        .with_state(VerifyState { sessions, admins })
}

#[derive(Debug, Deserialize)]
pub struct VerifyQuery {
    /// A role the session must have, exactly (as `require_role`).
    role: Option<String>,
}

async fn verify_handler(
    State(state): State<VerifyState>,
    Query(q): Query<VerifyQuery>,
    jar: CookieJar,
    headers: HeaderMap,
) -> Response {
    let role = match q.role.as_deref().map(Role::parse) {
        None => None,
        Some(Ok(role)) => Some(role),
        // A typo in the middleware's address must not open the gate.
        Some(Err(_)) => return no_store(StatusCode::FORBIDDEN.into_response()),
    };
    match current_session(&state.sessions, &jar).await {
        Some(session) if role.as_ref().is_none_or(|r| *r == session.role) => {
            identity(&state, &session).await
        }
        Some(_) => no_store(StatusCode::FORBIDDEN.into_response()),
        None if wants_page(&headers) => {
            redirect(&headers, &sign_in_then_back(forwarded_uri(&headers)))
        }
        None => no_store(StatusCode::UNAUTHORIZED.into_response()),
    }
}

/// `GET /auth/docs`: the documentation's pages and their assets alike go to
/// sign in, and come back to the page asked for.
async fn docs_handler(
    State(state): State<VerifyState>,
    jar: CookieJar,
    headers: HeaderMap,
) -> Response {
    match current_session(&state.sessions, &jar).await {
        Some(_) => StatusCode::NO_CONTENT.into_response(),
        None => redirect(&headers, &docs_sign_in(forwarded_uri(&headers))),
    }
}

/// `204` with who the session is. Every header goes, empty when there is no
/// value: Traefik replaces only the headers the gate sends, so a missing one
/// would let the client's own `X-Auth-*` through to the application.
async fn identity(state: &VerifyState, session: &AuthenticatedSession) -> Response {
    let email = match state.admins.find_by_id(&session.admin_id).await {
        Ok(Some(account)) => account.email.to_string(),
        _ => String::new(),
    };
    let org = tenancy::current().map(|t| t.slug).unwrap_or_default();
    let mut response = no_store(StatusCode::NO_CONTENT.into_response());
    let h = response.headers_mut();
    for (name, value) in [
        (AUTH_USER_HEADER, session.admin_id.as_str()),
        (AUTH_EMAIL_HEADER, email.as_str()),
        (AUTH_ROLE_HEADER, session.role.as_str()),
        (AUTH_ORG_HEADER, org.as_str()),
    ] {
        h.insert(
            name,
            HeaderValue::from_str(value).unwrap_or(HeaderValue::from_static("")),
        );
    }
    response
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

/// The request Traefik asks about (`X-Forwarded-Uri`).
fn forwarded_uri(headers: &HeaderMap) -> Option<&str> {
    headers.get("x-forwarded-uri").and_then(|v| v.to_str().ok())
}

/// Whether the request is a page a person is opening — then signing in makes
/// sense; a script's request gets `401` instead of a login page in its body.
fn wants_page(headers: &HeaderMap) -> bool {
    let method = headers
        .get("x-forwarded-method")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("GET");
    let html = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("text/html"));
    matches!(method, "GET" | "HEAD") && html
}

/// The sign-in screen, set to come back to the page asked for. Only a path of
/// this site (`next` must not send anyone elsewhere); `page=1` tells the
/// screen it is not one of its own routes, so it loads it whole.
fn sign_in_then_back(asked: Option<&str>) -> String {
    let back = asked
        .filter(|u| u.starts_with('/') && !u.starts_with("//") && !u.contains('\\'))
        .unwrap_or("/");
    format!("/login?next={}&page=1", encode(back))
}

/// The sign-in that comes back to that documentation page. Only a path under
/// `/docs/`: anything else goes back to the documentation's start (so `next`
/// cannot send anywhere else).
fn docs_sign_in(requested: Option<&str>) -> String {
    let target = requested
        .filter(|u| u.starts_with("/docs/") && !u.contains("//") && !u.contains('\\'))
        .unwrap_or("/docs/");
    format!("/login?next={}", encode(target))
}

fn encode(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    encoded
}

/// The site the person asked for (`X-Forwarded-Proto` and
/// `X-Forwarded-Host`, from Traefik), as `https://host`. Traefik resolves a
/// relative `Location` against the gate's own address (`http://api:8080`), so
/// the redirect has to name the site; without both headers it stays relative.
fn public_origin(headers: &HeaderMap) -> Option<String> {
    let get = |name| headers.get(name).and_then(|v| v.to_str().ok());
    let proto = get("x-forwarded-proto").filter(|p| matches!(*p, "https" | "http"))?;
    let host = get("x-forwarded-host").filter(|h| {
        !h.is_empty()
            && h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
    })?;
    Some(format!("{proto}://{host}"))
}

fn redirect(headers: &HeaderMap, path: &str) -> Response {
    let to = format!("{}{path}", public_origin(headers).unwrap_or_default());
    match HeaderValue::from_str(&to) {
        Ok(location) => {
            no_store((StatusCode::FOUND, [(header::LOCATION, location)]).into_response())
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comes_back_to_the_documentation_page() {
        assert_eq!(
            docs_sign_in(Some("/docs/instalacion/instalar/")),
            "/login?next=/docs/instalacion/instalar/"
        );
        assert_eq!(
            docs_sign_in(Some("/docs/uso/?q=a b#x")),
            "/login?next=/docs/uso/%3Fq%3Da%20b%23x"
        );
    }

    #[test]
    fn only_inside_the_documentation() {
        for outside in [
            None,
            Some("/settings"),
            Some("https://other.site/docs/"),
            Some("/docs//other.site"),
            Some("/docs/\\other.site"),
            Some("/docs"),
        ] {
            assert_eq!(docs_sign_in(outside), "/login?next=/docs/", "{outside:?}");
        }
    }

    #[test]
    fn verify_comes_back_to_a_path_of_this_site_only() {
        assert_eq!(
            sign_in_then_back(Some("/facturacion/?mes=2026-10")),
            "/login?next=/facturacion/%3Fmes%3D2026-10&page=1"
        );
        for outside in [
            None,
            Some("https://other.site/"),
            Some("//other.site/"),
            Some("/\\other.site"),
        ] {
            assert_eq!(
                sign_in_then_back(outside),
                "/login?next=/&page=1",
                "{outside:?}"
            );
        }
    }

    #[test]
    fn the_redirect_names_the_site_traefik_was_asked_for() {
        let mut headers = HeaderMap::new();
        assert_eq!(public_origin(&headers), None);
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        assert_eq!(public_origin(&headers), None);
        headers.insert(
            "x-forwarded-host",
            HeaderValue::from_static("admin.orquestador.app"),
        );
        assert_eq!(
            public_origin(&headers).as_deref(),
            Some("https://admin.orquestador.app")
        );
        headers.insert(
            "x-forwarded-host",
            HeaderValue::from_static("localhost:8443"),
        );
        assert_eq!(
            public_origin(&headers).as_deref(),
            Some("https://localhost:8443")
        );
        for odd in ["other.site/x", "a@other.site", "a b", ""] {
            headers.insert("x-forwarded-host", HeaderValue::from_static(odd));
            assert_eq!(public_origin(&headers), None, "{odd:?}");
        }
        headers.insert(
            "x-forwarded-host",
            HeaderValue::from_static("admin.orquestador.app"),
        );
        headers.insert("x-forwarded-proto", HeaderValue::from_static("javascript"));
        assert_eq!(public_origin(&headers), None);
    }

    #[test]
    fn only_a_page_goes_to_sign_in() {
        let mut headers = HeaderMap::new();
        assert!(!wants_page(&headers));
        headers.insert(header::ACCEPT, HeaderValue::from_static("text/html,*/*"));
        assert!(wants_page(&headers));
        headers.insert("x-forwarded-method", HeaderValue::from_static("POST"));
        assert!(!wants_page(&headers));
        headers.insert("x-forwarded-method", HeaderValue::from_static("GET"));
        headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
        assert!(!wants_page(&headers));
    }
}
