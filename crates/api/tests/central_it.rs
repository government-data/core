//! The online edition's common entrance (`entrar.<domain>`) with the real
//! tenant middleware and an ephemeral Postgres connected as in production (RLS
//! with FORCE): the email and password lead only to the companies where they
//! work, the pass is redeemed once on its subdomain, and a wrong password is
//! counted in all of them. Needs Docker.

use std::sync::Arc;
use std::time::Duration;

use auth_gateway::LoginService;
use auth_gateway::{AdminRepository, NewAdmin, PasswordHasher};
use auth_gateway::{AuditService, CentralLoginService, SessionService};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use domain::{Email, Role};
use infrastructure::{
    Argon2Hasher, Argon2Params, PgAdminRepository, PgAuditRepository, PgIpLockoutStore,
    PgLoginHandoffs, PgSessionRepository, PgTenantDirectory, ResetTokenAdapter, SecureRandomTokens,
    SystemClock,
};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use tenancy::Tenant;
use tower::ServiceExt;

async fn account(pool: &sqlx::PgPool, tenant: &str, slug: &str, email: &str, password: &str) {
    let hasher = Argon2Hasher::new(Argon2Params::owasp_default()).unwrap();
    let hash = hasher.hash(password).await.unwrap();
    let repo = PgAdminRepository::new(pool.clone());
    let tenant = Tenant {
        id: tenant.into(),
        slug: slug.into(),
    };
    tenancy::within(tenant, async {
        repo.insert(&NewAdmin {
            email: Email::parse(email).unwrap(),
            password_hash: hash,
            role: Role::admin(),
        })
        .await
        .unwrap();
    })
    .await;
}

async fn failed_attempts(db: &testkit::TestDb, slug: &str, email: &str) -> i32 {
    sqlx::query_scalar(
        "SELECT a.failed_attempts FROM admin_users a JOIN tenants t ON t.id = a.tenant_id \
          WHERE t.slug = $1 AND a.email = $2",
    )
    .bind(slug)
    .bind(email)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

fn app(pool: &sqlx::PgPool) -> axum::Router {
    let admins: Arc<dyn AdminRepository> = Arc::new(PgAdminRepository::new(pool.clone()));
    let directory = Arc::new(PgTenantDirectory::new(pool.clone()));
    let login = LoginService::new(
        admins.clone(),
        Arc::new(PgIpLockoutStore::new(pool.clone())),
        Arc::new(Argon2Hasher::new(Argon2Params::owasp_default()).unwrap()),
        Arc::new(SystemClock),
        auth_gateway::LockoutPolicy::recommended(),
    );
    let audit = AuditService::new(Arc::new(PgAuditRepository::new(pool.clone())));
    let service = CentralLoginService::new(
        login,
        directory.clone(),
        Arc::new(PgLoginHandoffs::new(pool.clone())),
        admins,
        Arc::new(ResetTokenAdapter),
        Arc::new(SystemClock),
        "main".into(),
        "orquestador.app".into(),
    )
    .with_audit(audit.clone());
    let sessions = SessionService::new(
        Arc::new(PgSessionRepository::new(pool.clone())),
        Arc::new(SecureRandomTokens),
        Arc::new(SystemClock),
        auth_gateway::SessionPolicy::recommended(),
    );
    api::central::routes(
        service,
        sessions,
        audit,
        api::rate_limit::RateLimitConfig {
            max_requests: 100,
            window: Duration::from_secs(60),
        },
    )
    .layer(axum::middleware::from_fn_with_state(
        api::tenancy::TenantResolver::multiple("orquestador.app", "main", directory),
        api::tenancy::resolve,
    ))
}

async fn post(
    app: &axum::Router,
    host: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value, bool) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(header::HOST, host)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let cookie = response.headers().contains_key(header::SET_COOKIE);
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        cookie,
    )
}

const ENTRANCE: &str = "entrar.orquestador.app";

#[tokio::test]
#[allow(clippy::too_many_lines)] // TODO(0034): split
async fn the_email_and_password_lead_only_to_the_companies_where_they_work() {
    let db = testkit::spawn_test_db().await;
    let andina = testkit::new_tenant(&db, "andina").await;
    let llanos = testkit::new_tenant(&db, "llanos").await;
    let sur = testkit::new_tenant(&db, "sur").await;
    let pool = infrastructure::db::with_tenancy(PgPoolOptions::new(), tenancy::Mode::Multiple)
        .connect_with(testkit::app_role(&db).await)
        .await
        .unwrap();
    account(&pool, &andina, "andina", "ana@coop.co", "Clave-de-Andina-1").await;
    account(&pool, &llanos, "llanos", "ana@coop.co", "Clave-de-Llanos-1").await;
    account(&pool, &sur, "sur", "otro@coop.co", "Clave-de-Andina-1").await;
    let app = app(&pool);

    // Andina's password: only Andina (nothing says the email is also in Llanos).
    let (status, body, _) = post(
        &app,
        ENTRANCE,
        "/auth/central",
        json!({ "email": "Ana@Coop.co", "password": "Clave-de-Andina-1" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let companies = body["companies"].as_array().unwrap();
    assert_eq!(companies.len(), 1);
    assert_eq!(companies[0]["slug"], "andina");
    let url = companies[0]["url"].as_str().unwrap();
    let token = url
        .strip_prefix("https://andina.orquestador.app/handoff#")
        .unwrap();
    // And no failure was counted in Llanos for not being its password.
    assert_eq!(failed_attempts(&db, "llanos", "ana@coop.co").await, 0);

    // The pass does not work in another company nor at the entrance; in its
    // own, once.
    for host in ["llanos.orquestador.app", ENTRANCE] {
        let (status, _, _) = post(&app, host, "/auth/handoff", json!({ "token": token })).await;
        assert_ne!(status, StatusCode::OK, "{host}");
    }
    let (status, body, cookie) = post(
        &app,
        "andina.orquestador.app",
        "/auth/handoff",
        json!({ "token": token }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(cookie, "la sesión va en la cookie del subdominio");
    let (status, _, _) = post(
        &app,
        "andina.orquestador.app",
        "/auth/handoff",
        json!({ "token": token }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A password that is nobody's: 401 and the failure in each company.
    let (status, _, _) = post(
        &app,
        ENTRANCE,
        "/auth/central",
        json!({ "email": "ana@coop.co", "password": "otra-cosa" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(failed_attempts(&db, "andina", "ana@coop.co").await, 1);
    assert_eq!(failed_attempts(&db, "llanos", "ana@coop.co").await, 1);
    assert_eq!(failed_attempts(&db, "sur", "otro@coop.co").await, 0);

    // An email that does not exist answers the same.
    let (status, _, _) = post(
        &app,
        ENTRANCE,
        "/auth/central",
        json!({ "email": "nadie@coop.co", "password": "Clave-de-Andina-1" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // With the same password in both, both (to choose).
    sqlx::query(
        "UPDATE admin_users SET password_hash = (SELECT password_hash FROM admin_users a \
           JOIN tenants t ON t.id = a.tenant_id WHERE t.slug = 'andina') \
          WHERE tenant_id = $1::uuid",
    )
    .bind(&llanos)
    .execute(&db.pool)
    .await
    .unwrap();
    let (status, body, _) = post(
        &app,
        ENTRANCE,
        "/auth/central",
        json!({ "email": "ana@coop.co", "password": "Clave-de-Andina-1" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let slugs: Vec<_> = body["companies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["slug"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(slugs, ["andina", "llanos"]);

    // Inside a company, /auth/central does not exist.
    let (status, _, _) = post(
        &app,
        "andina.orquestador.app",
        "/auth/central",
        json!({ "email": "ana@coop.co", "password": "Clave-de-Andina-1" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // No company can be called «entrar».
    assert!(
        sqlx::query("INSERT INTO tenants (slug, name) VALUES ('entrar', 'x')")
            .execute(&db.pool)
            .await
            .is_err()
    );
}
