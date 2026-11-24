//! The gate Traefik asks before serving an application (`/auth/verify`) and
//! a module's static site (`auth: forward`, the user guide at `/docs`),
//! against a real Postgres: with a session
//! it hands over who it is, without one a page goes to sign in and a script
//! gets 401, and `?role=` keeps out the other roles. Requires Docker.

use std::sync::Arc;
use std::time::Duration;

use auth_gateway::{
    AdminRepository, AuditService, LoginService, NewAdmin, PasswordHasher, SessionService,
    DIRECTORY_USER_ROLE,
};
use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
use domain::{Email, Role};
use infrastructure::{
    Argon2Hasher, Argon2Params, PgAdminRepository, PgAuditRepository, PgIpLockoutStore,
    PgSessionRepository, SecureRandomTokens, SystemClock,
};
use tower::ServiceExt;

const PASSWORD: &str = "Str0ngEnoughPass";

async fn app(pool: &sqlx::PgPool) -> axum::Router {
    let admins: Arc<dyn AdminRepository> = Arc::new(PgAdminRepository::new(pool.clone()));
    let hasher = Argon2Hasher::new(Argon2Params::owasp_default()).unwrap();
    for (email, role) in [
        ("ana@empresa.com", Role::admin()),
        ("leo@empresa.com", Role::parse(DIRECTORY_USER_ROLE).unwrap()),
    ] {
        admins
            .insert(&NewAdmin {
                email: Email::parse(email).unwrap(),
                password_hash: hasher.hash(PASSWORD).await.unwrap(),
                role,
            })
            .await
            .unwrap();
    }
    let login = LoginService::new(
        admins.clone(),
        Arc::new(PgIpLockoutStore::new(pool.clone())),
        Arc::new(hasher),
        Arc::new(SystemClock),
        auth_gateway::LockoutPolicy::recommended(),
    );
    let sessions = SessionService::new(
        Arc::new(PgSessionRepository::new(pool.clone())),
        Arc::new(SecureRandomTokens),
        Arc::new(SystemClock),
        auth_gateway::SessionPolicy::recommended(),
    );
    let audit = AuditService::new(Arc::new(PgAuditRepository::new(pool.clone())));
    api::auth::routes(
        login,
        sessions.clone(),
        audit,
        api::rate_limit::RateLimitConfig {
            max_requests: 100,
            window: Duration::from_secs(60),
        },
    )
    .merge(api::verify::routes(sessions, admins))
}

async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, HeaderMap) {
    let response = app.clone().oneshot(req).await.unwrap();
    (response.status(), response.headers().clone())
}

async fn session_cookie(app: &axum::Router, email: &str) -> String {
    let (status, headers) = send(
        app,
        Request::post("/auth/login")
            .header("content-type", "application/json")
            .header("x-forwarded-for", "203.0.113.7")
            .body(Body::from(
                serde_json::json!({"email": email, "password": PASSWORD}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string())
        .find(|c| c.starts_with("session="))
        .expect("a session cookie")
}

/// What Traefik sends the gate: the original request's headers plus where
/// it was going.
fn asked(path: &str, uri: &str) -> axum::http::request::Builder {
    Request::get(path)
        .header("x-forwarded-method", "GET")
        .header("x-forwarded-uri", uri)
}

#[tokio::test]
async fn with_a_session_the_gate_hands_over_who_it_is() {
    let db = testkit::spawn_test_db().await;
    let app = app(&db.pool).await;
    let cookie = session_cookie(&app, "ana@empresa.com").await;

    let (status, headers) = send(
        &app,
        asked("/auth/verify", "/facturacion/")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(headers["x-auth-email"], "ana@empresa.com");
    assert_eq!(headers["x-auth-role"], "admin");
    assert!(!headers["x-auth-user"].is_empty());
    // No company header any more (decision 0076).
    assert!(!headers.contains_key("x-auth-org"));

    let (status, _) = send(
        &app,
        asked("/auth/verify?role=admin", "/facturacion/")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn without_a_session_a_page_goes_to_sign_in_and_a_script_gets_401() {
    let db = testkit::spawn_test_db().await;
    let app = app(&db.pool).await;

    let (status, headers) = send(
        &app,
        asked("/auth/verify", "/facturacion/?mes=10")
            .header(header::ACCEPT, "text/html,application/xhtml+xml")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers[header::LOCATION],
        "/login?next=/facturacion/%3Fmes%3D10&page=1"
    );

    let (status, headers) = send(
        &app,
        asked("/auth/verify", "/facturacion/api/pagos")
            .header(header::ACCEPT, "application/json")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(headers.get(header::LOCATION).is_none());

    // Behind Traefik, the redirect names the site: Traefik would resolve a
    // relative one against the gate's address (http://api:8080).
    let (status, headers) = send(
        &app,
        asked("/auth/verify", "/docs/uso/")
            .header(header::ACCEPT, "text/html")
            .header("x-forwarded-proto", "https")
            .header("x-forwarded-host", "quality.example.com")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers[header::LOCATION],
        "https://quality.example.com/login?next=/docs/uso/&page=1"
    );
}

#[tokio::test]
async fn a_role_the_session_lacks_is_refused() {
    let db = testkit::spawn_test_db().await;
    let app = app(&db.pool).await;
    let cookie = session_cookie(&app, "leo@empresa.com").await;

    for path in ["/auth/verify?role=admin", "/auth/verify?role="] {
        let (status, headers) = send(
            &app,
            asked(path, "/facturacion/")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        assert!(headers.get("x-auth-user").is_none());
    }
    // Without a role asked for, any session goes.
    let (status, _) = send(
        &app,
        asked("/auth/verify", "/facturacion/")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}
