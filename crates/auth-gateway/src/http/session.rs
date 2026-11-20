//! HTTP boundary for sessions: the cookies a login response sets, the
//! middleware that authenticates them (and checks CSRF on mutations) for every
//! protected route, and the `/auth/logout` endpoint that revokes one.
//!
//! Holds no business logic — every decision (session validity, expiry, CSRF
//! matching) lives in [`SessionService`]; this module only speaks HTTP and
//! cookies.

use std::time::SystemTime;

use std::sync::Arc;

use crate::{AdminRepository, SessionToken};
use crate::{AuditService, IssuedSession, SessionError, SessionService};
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use domain::{AdminId, AuditEventType, NewAuditEvent, Role};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::http::auth::ClientIp;
use http_kit::error::ApiError;

/// The `HttpOnly` cookie carrying the session bearer token.
pub const SESSION_COOKIE: &str = "session";
/// The client-readable cookie carrying the CSRF token, mirrored back by the
/// caller in [`CSRF_HEADER`] on every mutation.
pub const CSRF_COOKIE: &str = "csrf";
/// The request header a mutating call must echo the CSRF cookie's value into.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// State for the session-protected routes: the session service (logout +
/// auth middleware), the audit trail (logout), and the admin repository (me).
#[derive(Clone)]
pub struct SessionRouteState {
    sessions: SessionService,
    audit: AuditService,
    admins: Arc<dyn AdminRepository>,
    version: AppVersion,
}

impl std::fmt::Debug for SessionRouteState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionRouteState").finish_non_exhaustive()
    }
}

/// The running version, for `/auth/me`: whoever mounts the routes sets it (the
/// tag's and, if built with it, the commit).
#[derive(Debug, Clone, Copy)]
pub struct AppVersion {
    pub version: &'static str,
    pub commit: Option<&'static str>,
}

/// Mount the routes that require an authenticated session: `/auth/me` (any
/// role) and `/auth/logout`. Future protected mutations mount alongside these
/// the same way.
pub fn routes(
    sessions: SessionService,
    audit: AuditService,
    admins: Arc<dyn AdminRepository>,
    version: AppVersion,
) -> Router {
    Router::new()
        .route("/auth/logout", post(logout_handler))
        .route("/auth/me", get(me_handler))
        .route("/auth/me/locale", axum::routing::put(locale_handler))
        .route(
            "/auth/me/avatar",
            get(avatar_handler)
                .put(upload_avatar_handler)
                .delete(remove_avatar_handler),
        )
        .with_state(SessionRouteState {
            sessions: sessions.clone(),
            audit,
            admins,
            version,
        })
        .layer(axum::middleware::from_fn_with_state(
            sessions,
            require_session,
        ))
}

/// The authenticated identity a protected handler can extract from request
/// extensions, populated by [`require_session`].
#[derive(Debug, Clone)]
pub struct CurrentSession {
    /// The authenticated administrator's id.
    pub admin_id: String,
    /// The authenticated administrator's role — the same value `/auth/me`
    /// reports and [`crate::http::rbac::require_role`] checks.
    pub role: Role,
    /// The raw session token, so a handler that ends the session (logout,
    /// rotation) can act on the exact row without a second cookie read.
    /// Empty for a service account's token.
    pub token: SessionToken,
    /// The service account the request comes with (`Authorization: Bearer`),
    /// acting for its owner `admin_id` (ADR 0016).
    pub service_account: Option<String>,
}

impl CurrentSession {
    /// Whether the request may make or touch an `admin`: an `admin`'s own
    /// session, never a service account's token, whose policies may allow
    /// less than its owner's role.
    #[must_use]
    pub fn acts_as_admin(&self) -> bool {
        self.service_account.is_none() && self.role == Role::admin()
    }
}

/// Axum middleware: authenticate the `session` cookie and, for a mutating
/// method, verify the `X-CSRF-Token` header against it. Rejects with `401`
/// (no/invalid/expired session) or `403` (CSRF mismatch) before the wrapped
/// handler ever runs; on success, injects a [`CurrentSession`] the handler can
/// extract.
///
/// A service account's token (`Authorization: Bearer <access key>.<secret>`)
/// is accepted instead of the cookie. It needs no CSRF token: a browser never
/// sends it by itself.
pub async fn require_session(
    State(sessions): State<SessionService>,
    jar: CookieJar,
    request: Request,
    next: Next,
) -> Response {
    let bearer = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string());
    if let Some(bearer) = bearer {
        return match sessions.authenticate_service(&bearer).await {
            Ok(Some(principal)) => {
                let mut request = request;
                request.extensions_mut().insert(CurrentSession {
                    admin_id: principal.owner_id.as_str().to_string(),
                    role: principal.owner_role,
                    token: SessionToken::from_raw(""),
                    service_account: Some(principal.service_account_id),
                });
                next.run(request).await
            }
            Ok(None) => unauthorized(),
            Err(e) => {
                tracing::warn!("service token check failed: {e}");
                unauthorized()
            }
        };
    }

    let Some(token) = jar
        .get(SESSION_COOKIE)
        .map(|c| SessionToken::from_raw(c.value()))
    else {
        return unauthorized();
    };

    let authenticated = match sessions.authenticate(&token).await {
        Ok(a) => a,
        Err(SessionError::NotFound | SessionError::Expired) => return unauthorized(),
        Err(_) => return unauthorized(),
    };

    let is_mutating = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if is_mutating {
        let header_value = request
            .headers()
            .get(CSRF_HEADER)
            .and_then(|v| v.to_str().ok());
        if sessions.verify_csrf(&authenticated, header_value).is_err() {
            return forbidden();
        }
    }

    let mut request = request;
    request.extensions_mut().insert(CurrentSession {
        admin_id: authenticated.admin_id.as_str().to_string(),
        role: authenticated.role,
        token,
        service_account: None,
    });
    next.run(request).await
}

fn unauthorized() -> Response {
    ApiError::unauthorized().into_response()
}

fn forbidden() -> Response {
    ApiError::forbidden(
        "csrf_mismatch",
        "The CSRF token is missing or does not match.",
    )
    .into_response()
}

/// Build the `Set-Cookie` pair for a freshly issued session: the `HttpOnly`
/// session cookie and the client-readable CSRF cookie, both `Secure`,
/// `SameSite=Strict`, scoped to `/`, and capped at the session's absolute
/// expiry.
///
/// `SameSite=Strict` (rather than `Lax`) is safe here because nothing in this
/// admin panel needs the cookie sent on a cross-site top-level navigation: an
/// admin always reaches the app by typing the URL or via an already-open tab,
/// never by following a link from another site into an authenticated page.
pub fn attach_session_cookies(jar: CookieJar, issued: &IssuedSession) -> CookieJar {
    let expires = to_offset_date_time(issued.absolute_expires_at);

    let session_cookie = Cookie::build((SESSION_COOKIE, issued.token.as_str().to_string()))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Strict)
        .path("/")
        .expires(expires)
        .build();

    // Not `HttpOnly`: client-side script must read it to mirror it into the
    // `X-CSRF-Token` header on mutating requests.
    let csrf_cookie = Cookie::build((CSRF_COOKIE, issued.csrf_token.as_str().to_string()))
        .http_only(false)
        .secure(true)
        .same_site(SameSite::Strict)
        .path("/")
        .expires(expires)
        .build();

    jar.add(session_cookie).add(csrf_cookie)
}

/// Clear both session cookies (logout): same name/path/flags with an
/// already-expired deadline, which is what makes the removal stick across
/// browsers.
/// Expire the session and CSRF cookies (logout).
pub fn clear_session_cookies(jar: CookieJar) -> CookieJar {
    let expired = Cookie::build((SESSION_COOKIE, ""))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Strict)
        .path("/")
        .expires(OffsetDateTime::UNIX_EPOCH)
        .build();
    let expired_csrf = Cookie::build((CSRF_COOKIE, ""))
        .http_only(false)
        .secure(true)
        .same_site(SameSite::Strict)
        .path("/")
        .expires(OffsetDateTime::UNIX_EPOCH)
        .build();
    jar.add(expired).add(expired_csrf)
}

fn to_offset_date_time(time: SystemTime) -> OffsetDateTime {
    OffsetDateTime::from(time)
}

/// The body `GET /auth/me` returns: enough for the frontend to know who is
/// logged in and which role's guards to apply, plus profile fields for the UI.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MeOut {
    /// The authenticated administrator's opaque id.
    admin_id: String,
    /// The administrator's role (e.g. `admin`).
    role: String,
    /// The administrator's email address.
    email: String,
    /// An optional display name for the profile UI.
    display_name: Option<String>,
    /// The server's version (the release tag), shown in the UI for debugging.
    version: String,
    /// The commit the server was built from, when it is a release build.
    commit: Option<String>,
    /// The interface language the admin chose (`es` / `en`); `null` follows
    /// the browser.
    locale: Option<String>,
    /// The profile picture's version (when it changed, Unix seconds), to load
    /// it from `/auth/me/avatar?v=…`; `null` without one.
    avatar: Option<i64>,
}

/// The body of `PUT /auth/me/locale`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct LocaleIn {
    /// `es`, `en`, or `null` to follow the browser again.
    locale: Option<String>,
}

/// Handle `GET /auth/me`: report the authenticated admin's identity and
/// profile fields, so the frontend can display them without a separate round
/// trip. Any authenticated role may call this — it is not itself role-gated
/// (see [`require_role`](crate::http::rbac::require_role) for endpoints that are).
#[utoipa::path(
    get,
    path = "/auth/me",
    responses(
        (status = 200, description = "The authenticated admin's identity and profile", body = MeOut),
        (status = 401, description = "No valid session"),
    ),
    tag = "auth",
)]
pub async fn me_handler(
    State(state): State<SessionRouteState>,
    current: axum::Extension<CurrentSession>,
) -> impl IntoResponse {
    let admin_id = AdminId::new(&current.0.admin_id);
    let account = state.admins.find_by_id(&admin_id).await;

    let (email, display_name) = match account {
        Ok(Some(a)) => (a.email.to_string(), a.display_name),
        _ => (String::new(), None),
    };

    let locale = state.admins.locale(&admin_id).await.ok().flatten();
    let avatar = state.admins.avatar_version(&admin_id).await.ok().flatten();
    Json(MeOut {
        locale,
        avatar,
        admin_id: current.0.admin_id.clone(),
        role: current.0.role.as_str().to_string(),
        email,
        display_name,
        version: state.version.version.to_string(),
        commit: state.version.commit.map(str::to_string),
    })
}

/// Handle `GET /auth/me/avatar`: the admin's profile picture. Its URL carries
/// the version (`?v=`), so the browser may keep it for a long time.
pub async fn avatar_handler(
    State(state): State<SessionRouteState>,
    current: axum::Extension<CurrentSession>,
) -> axum::response::Response {
    match state
        .admins
        .avatar(&AdminId::new(&current.0.admin_id))
        .await
    {
        Ok(Some(p)) if crate::ProfilePicture::accepts(&p.content_type) => (
            [
                (axum::http::header::CONTENT_TYPE, p.content_type),
                (
                    axum::http::header::CACHE_CONTROL,
                    "private, max-age=31536000, immutable".to_string(),
                ),
                (
                    axum::http::header::X_CONTENT_TYPE_OPTIONS,
                    "nosniff".to_string(),
                ),
            ],
            p.bytes,
        )
            .into_response(),
        Ok(_) => axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => ApiError::internal(format!("avatar: {e}")).into_response(),
    }
}

/// The image type the bytes really are (by their first bytes), whatever the
/// request says: what is kept and served is never something else.
fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, ..] => Some("image/png"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        _ => None,
    }
}

/// Handle `PUT /auth/me/avatar`: the admin's own profile picture, as the body
/// (JPEG, PNG, WebP or GIF, up to [`ProfilePicture::MAX_BYTES`]).
pub async fn upload_avatar_handler(
    State(state): State<SessionRouteState>,
    current: axum::Extension<CurrentSession>,
    body: axum::body::Bytes,
) -> Response {
    if body.len() > crate::ProfilePicture::MAX_BYTES {
        return ApiError::bad_request(
            "too_large",
            format!(
                "la foto pesa {} KB: el máximo es {} KB (achícala o recórtala)",
                body.len() / 1024,
                crate::ProfilePicture::MAX_BYTES / 1024
            ),
        )
        .into_response();
    }
    let Some(content_type) = sniff_image(&body) else {
        return ApiError::bad_request(
            "not_an_image",
            "la foto tiene que ser una imagen JPEG, PNG, WebP o GIF",
        )
        .into_response();
    };
    let picture = crate::ProfilePicture {
        content_type: content_type.to_string(),
        bytes: body.to_vec(),
    };
    match state
        .admins
        .set_own_avatar(&AdminId::new(&current.0.admin_id), Some(&picture))
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => ApiError::internal(format!("avatar: {e}")).into_response(),
    }
}

/// Handle `DELETE /auth/me/avatar`: drop the admin's profile picture (the
/// initial shows instead).
pub async fn remove_avatar_handler(
    State(state): State<SessionRouteState>,
    current: axum::Extension<CurrentSession>,
) -> Response {
    match state
        .admins
        .set_own_avatar(&AdminId::new(&current.0.admin_id), None)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => ApiError::internal(format!("avatar: {e}")).into_response(),
    }
}

/// Handle `PUT /auth/me/locale`: remember the admin's interface language.
#[utoipa::path(
    put,
    path = "/auth/me/locale",
    request_body = LocaleIn,
    responses(
        (status = 204, description = "Saved"),
        (status = 401, description = "No valid session"),
        (status = 422, description = "Not a supported language"),
    ),
    tag = "auth",
)]
pub async fn locale_handler(
    State(state): State<SessionRouteState>,
    current: axum::Extension<CurrentSession>,
    Json(body): Json<LocaleIn>,
) -> Response {
    let locale = body
        .locale
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty());
    if let Some(l) = locale {
        if !["es", "en"].contains(&l) {
            let mut fields = std::collections::BTreeMap::new();
            fields.insert("locale".to_string(), "must be es or en".to_string());
            return ApiError::validation(fields).into_response();
        }
    }
    match state
        .admins
        .set_locale(&AdminId::new(&current.0.admin_id), locale)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => ApiError::internal(format!("locale: {e}")).into_response(),
    }
}

/// Handle `POST /auth/logout`: revoke the session server-side, record the
/// event to the audit trail, and clear both cookies. Requires (via
/// [`require_session`]) a valid session and a matching CSRF header — logout
/// is a mutation like any other.
#[utoipa::path(
    post,
    path = "/auth/logout",
    responses(
        (status = 204, description = "Logged out; clears the session and csrf cookies"),
        (status = 401, description = "No valid session"),
        (status = 403, description = "Missing or mismatched CSRF header"),
    ),
    tag = "auth",
)]
pub async fn logout_handler(
    State(state): State<SessionRouteState>,
    current: axum::Extension<CurrentSession>,
    ClientIp(client_ip): ClientIp,
    headers: HeaderMap,
    jar: CookieJar,
) -> impl IntoResponse {
    // Best-effort: an already-gone session (e.g. it just expired) is not an
    // error from the caller's point of view — the outcome they wanted (no
    // longer logged in) already holds.
    let _ = state.sessions.revoke(&current.0.token).await;

    let event = NewAuditEvent {
        event_type: AuditEventType::LoggedOut,
        admin_id: Some(AdminId::new(current.0.admin_id.clone())),
        email_attempted: None,
        ip: client_ip,
        user_agent: headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
        occurred_at: SystemTime::now(),
    };
    // Best-effort, same reasoning as the login handler: an audit-store outage
    // must never block a logout.
    if let Err(e) = state.audit.record(event).await {
        tracing::warn!("logout: failed to record audit event: {e}");
    }

    (StatusCode::NO_CONTENT, clear_session_cookies(jar))
}
