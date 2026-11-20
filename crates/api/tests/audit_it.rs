//! End-to-end test for the audit trail (bead authapp-c418dc): drives the real
//! router — login, the audit query endpoint, and logout — against a real,
//! ephemeral, freshly migrated Postgres via `testkit`, and confirms every
//! step is recorded and that reading the trail itself requires a session.

use std::sync::Arc;
use std::time::Duration;

use application::HealthService;
use auth_gateway::{AuditService, SessionService};
use auth_gateway::{BootstrapService, LoginService};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::{
    Argon2Hasher, Argon2Params, PgAdminRepository, PgAuditRepository, PgHealthCheck,
    PgIpLockoutStore, PgSessionRepository, SecureRandomTokens, SystemClock,
};
use tower::ServiceExt;

const EMAIL: &str = "admin@example.com";
const PASSWORD: &str = "Str0ngEnoughPassphrase!";

async fn router(pool: sqlx::PgPool) -> axum::Router {
    let health = HealthService::new(Arc::new(PgHealthCheck::new(pool.clone())));
    let login = LoginService::new(
        Arc::new(PgAdminRepository::new(pool.clone())),
        Arc::new(PgIpLockoutStore::new(pool.clone())),
        Arc::new(Argon2Hasher::new(Argon2Params::owasp_default()).unwrap()),
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
    api::router(
        health,
        login,
        sessions,
        audit,
        Arc::new(PgAdminRepository::new(pool.clone())),
        None,
        application::PlatformService::new(Arc::new(infrastructure::PgPlatformRepository::new(
            pool.clone(),
        ))),
        None,
        None,
        &[],
        api::rate_limit::RateLimitConfig {
            max_requests: 100,
            window: Duration::from_secs(60),
        },
    )
}

async fn bootstrap_admin(pool: sqlx::PgPool) {
    let bootstrap = BootstrapService::new(
        Arc::new(PgAdminRepository::new(pool)),
        Arc::new(Argon2Hasher::new(Argon2Params::owasp_default()).unwrap()),
        auth_gateway::PasswordPolicy::recommended(),
    );
    bootstrap.create_first_admin(EMAIL, PASSWORD).await.unwrap();
}

fn login_body() -> Body {
    Body::from(serde_json::json!({"email": EMAIL, "password": PASSWORD}).to_string())
}

/// Pull `name=value` out of one or more `Set-Cookie` response headers.
fn cookie_value(response: &axum::http::Response<Body>, name: &str) -> String {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .find_map(|v| {
            let s = v.to_str().ok()?;
            let (pair, _rest) = s.split_once(';')?;
            let (k, v) = pair.split_once('=')?;
            (k == name).then(|| v.to_string())
        })
        .unwrap_or_else(|| panic!("no {name} cookie in response"))
}

#[tokio::test]
async fn audit_events_require_a_session_and_record_login_success() {
    let db = testkit::spawn_test_db().await;
    bootstrap_admin(db.pool.clone()).await;
    let app = router(db.pool.clone()).await;

    // Unauthenticated: the trail itself is gated.
    let unauthenticated = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/audit/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    // Log in successfully.
    let login_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("content-type", "application/json")
                .body(login_body())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login_response.status(), StatusCode::OK);
    let session_cookie = cookie_value(&login_response, "session");

    // Authenticated: the trail includes the login we just performed.
    let events_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/audit/events")
                .header("cookie", format!("session={session_cookie}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(events_response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(events_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let events: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let events = events.as_array().unwrap();
    assert!(events
        .iter()
        .any(|e| e["event_type"] == "login_succeeded" && e["email_attempted"] == EMAIL));
}

#[tokio::test]
async fn failed_login_is_recorded_without_leaking_a_password() {
    let db = testkit::spawn_test_db().await;
    bootstrap_admin(db.pool.clone()).await;
    let app = router(db.pool.clone()).await;

    // A wrong password.
    let failed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"email": EMAIL, "password": "wrong"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(failed.status(), StatusCode::UNAUTHORIZED);

    // A real session, so the events endpoint answers.
    let login_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("content-type", "application/json")
                .body(login_body())
                .unwrap(),
        )
        .await
        .unwrap();
    let session_cookie = cookie_value(&login_response, "session");

    let events_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/audit/events")
                .header("cookie", format!("session={session_cookie}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(events_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let events: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let events = events.as_array().unwrap();

    let failed_event = events
        .iter()
        .find(|e| e["event_type"] == "login_failed")
        .expect("the failed login must be recorded");
    // No password anywhere in the record — only the JSON keys this type
    // defines exist, and none of them can hold one (see `NewAuditEvent`).
    let serialized = failed_event.to_string();
    assert!(!serialized.contains("wrong"));
}

#[tokio::test]
async fn logout_is_recorded() {
    let db = testkit::spawn_test_db().await;
    bootstrap_admin(db.pool.clone()).await;
    let app = router(db.pool.clone()).await;

    let login_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("content-type", "application/json")
                .body(login_body())
                .unwrap(),
        )
        .await
        .unwrap();
    let session_cookie = cookie_value(&login_response, "session");
    let csrf_cookie = cookie_value(&login_response, "csrf");

    let logout_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/logout")
                .header(
                    "cookie",
                    format!("session={session_cookie}; csrf={csrf_cookie}"),
                )
                .header("x-csrf-token", &csrf_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout_response.status(), StatusCode::NO_CONTENT);

    // Log in again (the old session is gone) so the events endpoint answers.
    let second_login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("content-type", "application/json")
                .body(login_body())
                .unwrap(),
        )
        .await
        .unwrap();
    let second_session_cookie = cookie_value(&second_login, "session");

    let events_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/audit/events")
                .header("cookie", format!("session={second_session_cookie}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(events_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let events: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let events = events.as_array().unwrap();
    assert!(events.iter().any(|e| e["event_type"] == "logged_out"));
}

/// ADR 0013: viewing the log is the `view_audit` capability, not the `admin`
/// role. An account with a role that has it gets in; with one that does not,
/// `403`.
#[tokio::test]
async fn viewing_the_audit_log_is_an_action() {
    let db = testkit::spawn_test_db().await;
    bootstrap_admin(db.pool.clone()).await;
    let app = router(db.pool.clone()).await;
    // Two users with the administrator's password: one with a policy without
    // the audit log, the other with `auditor` (ADR 0016).
    for (email, policy) in [
        ("viewer@example.com", "quality-viewer"),
        ("auditor@example.com", "auditor"),
    ] {
        let id: String = sqlx::query_scalar(
            "INSERT INTO admin_users (email, password_hash, role)
             SELECT $1, password_hash, 'user' FROM admin_users WHERE email = $2
             RETURNING id::text",
        )
        .bind(email)
        .bind(EMAIL)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO iam_attachments (principal_kind, principal_id, policy)
             VALUES ('user', $1::uuid, $2)",
        )
        .bind(id)
        .bind(policy)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    let view = |email: &'static str| {
        let app = app.clone();
        async move {
            let r = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/auth/login")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            serde_json::json!({"email": email, "password": PASSWORD}).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::OK);
            let cookie = cookie_value(&r, "session");
            app.oneshot(
                Request::builder()
                    .uri("/audit/events")
                    .header("cookie", format!("session={cookie}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
        }
    };
    assert_eq!(view("viewer@example.com").await, StatusCode::FORBIDDEN);
    assert_eq!(view("auditor@example.com").await, StatusCode::OK);
}

/// Decision 0069: a view's reason and shape are listed with it, and an event
/// that is not a view has none.
#[tokio::test]
async fn a_view_is_listed_with_its_reason_and_shape() {
    use domain::{AuditEventType, AuditRepository, NewActionAudit, NewViewAudit, ViewShown};

    let db = testkit::spawn_test_db().await;
    bootstrap_admin(db.pool.clone()).await;
    let app = router(db.pool.clone()).await;
    let job = "00000000-0000-0000-0000-000000000003";
    PgAuditRepository::new(db.pool.clone())
        .record_view(&NewViewAudit {
            action: NewActionAudit {
                event_type: AuditEventType::RecordsViewed,
                admin_id: None,
                service_account: None,
                job_id: Some(job.to_string()),
                action: "quality:ViewPersonalData".to_string(),
                resource: "lake/console/1".to_string(),
                ip: "203.0.113.1".to_string(),
                user_agent: None,
                occurred_at: std::time::SystemTime::now() - Duration::from_secs(60),
            },
            shown: ViewShown {
                purpose: "other".to_string(),
                reference: Some("INC-2041".to_string()),
                rows_shown: 7,
                fields_shown: vec!["cliente_id".to_string(), "#2".to_string()],
            },
        })
        .await
        .unwrap();
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header("content-type", "application/json")
                .body(login_body())
                .unwrap(),
        )
        .await
        .unwrap();
    let cookie = cookie_value(&login, "session");
    let response = app
        .oneshot(
            Request::builder()
                .uri("/audit/events")
                .header("cookie", format!("session={cookie}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let events: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let events = events.as_array().unwrap();
    let view = events
        .iter()
        .find(|e| e["event_type"] == "records_viewed")
        .expect("the view is listed");
    assert_eq!(view["job_id"], job);
    assert_eq!(view["purpose"], "other");
    assert_eq!(view["reference"], "INC-2041");
    assert_eq!(view["rows_shown"], 7);
    assert_eq!(
        view["fields_shown"],
        serde_json::json!(["cliente_id", "#2"])
    );
    let login = events
        .iter()
        .find(|e| e["event_type"] == "login_succeeded")
        .expect("the login is listed");
    for field in ["purpose", "reference", "rows_shown", "fields_shown"] {
        assert!(login[field].is_null(), "{field} of a login");
    }
}
