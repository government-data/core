//! The gateway as an OIDC provider, end to end against a real Postgres: a
//! person signs in, an application sends them to `/oauth/authorize`, gets a
//! code, redeems it once at `/oauth/token`, and checks the id_token with the
//! key the gateway publishes — as any relying party would. Requires Docker.

use std::sync::Arc;
use std::time::Duration;

use auth_gateway::{
    AdminRepository, AuditService, AuthorizationServer, ClientAuth, ClientId, LoginService,
    NewAdmin, OidcClient, OidcClientRegistry, PasswordHasher, SessionService,
};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use domain::{Email, Role};
use infrastructure::oidc::{Ed25519Signer, PgAccountPolicies, PgAuthorizationCodes, PgOidcClients};
use infrastructure::oidc_es256::Es256Signer;
use infrastructure::{
    Argon2Hasher, Argon2Params, OAuthSecrets, PgAdminRepository, PgAuditRepository,
    PgIpLockoutStore, PgSessionRepository, ResetTokenAdapter, SecureRandomTokens, SystemClock,
};
use ring::signature::{UnparsedPublicKey, ED25519};
use serde_json::Value;
use tower::ServiceExt;

const ISSUER: &str = "https://orq.example.com/api";
const REDIRECT: &str = "https://gestion.example.com/callback";
// RFC 7636, appendix B.
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

async fn app(pool: &sqlx::PgPool) -> axum::Router {
    let admins: Arc<dyn AdminRepository> = Arc::new(PgAdminRepository::new(pool.clone()));
    let hasher = Argon2Hasher::new(Argon2Params::owasp_default()).unwrap();
    admins
        .insert(&NewAdmin {
            email: Email::parse("ana@empresa.com").unwrap(),
            password_hash: hasher.hash("Str0ngEnoughPass").await.unwrap(),
            role: Role::admin(),
        })
        .await
        .unwrap();
    PgOidcClients::new(pool.clone())
        .insert(&OidcClient {
            id: ClientId::parse("gestion").unwrap(),
            name: "Gestión".into(),
            redirect_uris: vec![REDIRECT.into()],
            auth: ClientAuth::Public,
            settings: auth_gateway::ClientSettings::default(),
        })
        .await
        .unwrap();

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
    let signer = Ed25519Signer::from_pkcs8_base64(&Ed25519Signer::generate().unwrap()).unwrap();
    let server = AuthorizationServer::new(
        Arc::new(PgOidcClients::new(pool.clone())),
        Arc::new(PgAuthorizationCodes::new(pool.clone())),
        Arc::new(ResetTokenAdapter),
        Arc::new(OAuthSecrets),
        Arc::new(signer),
        admins,
        Arc::new(SystemClock),
        ISSUER,
    )
    .with_signer(Arc::new(
        Es256Signer::from_pkcs8_base64(&Es256Signer::generate().unwrap()).unwrap(),
    ))
    .with_policies(Arc::new(PgAccountPolicies::new(pool.clone())));
    api::auth::routes(
        login,
        sessions.clone(),
        audit,
        api::rate_limit::RateLimitConfig {
            max_requests: 100,
            window: Duration::from_secs(60),
        },
    )
    .merge(auth_gateway::http::provider::routes(server, sessions))
}

async fn send(
    app: &axum::Router,
    req: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap()
        .to_vec();
    (status, headers, body)
}

async fn session_cookie(app: &axum::Router) -> String {
    let (status, headers, _) = send(
        app,
        Request::post("/auth/login")
            .header("content-type", "application/json")
            .header("x-forwarded-for", "203.0.113.7")
            .body(Body::from(
                serde_json::json!({"email": "ana@empresa.com", "password": "Str0ngEnoughPass"})
                    .to_string(),
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

fn authorize_uri() -> String {
    format!(
        "/oauth/authorize?response_type=code&client_id=gestion\
         &redirect_uri=https%3A%2F%2Fgestion.example.com%2Fcallback\
         &scope=openid%20email&state=xyz&nonce=n-1\
         &code_challenge={CHALLENGE}&code_challenge_method=S256"
    )
}

fn token_request(code: &str) -> Request<Body> {
    Request::post("/oauth/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(format!(
            "grant_type=authorization_code&code={code}&client_id=gestion\
             &redirect_uri=https%3A%2F%2Fgestion.example.com%2Fcallback\
             &code_verifier={VERIFIER}"
        )))
        .unwrap()
}

fn json(body: &[u8]) -> Value {
    serde_json::from_slice(body).unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // TODO(0034): split
async fn an_application_signs_a_person_in_and_checks_the_id_token_with_the_published_key() {
    let db = testkit::spawn_test_db().await;
    let app = app(&db.pool).await;

    // Discovery and keys, under the issuer.
    let (status, _, body) = send(
        &app,
        Request::get("/.well-known/openid-configuration")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body)["issuer"], ISSUER);
    let (_, _, jwks) = send(
        &app,
        Request::get("/oauth/jwks").body(Body::empty()).unwrap(),
    )
    .await;
    let jwks = json(&jwks);

    // Sent from another site without the (SameSite=Strict) cookie: the page
    // that repeats the request from this one.
    let (status, _, body) = send(
        &app,
        Request::get(authorize_uri())
            .header("sec-fetch-site", "cross-site")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8(body)
        .unwrap()
        .contains("http-equiv=refresh"));

    // Still without a session: to sign in, and back.
    let (status, headers, _) = send(
        &app,
        Request::get(authorize_uri())
            .header("sec-fetch-site", "same-origin")
            .header("x-forwarded-prefix", "/api")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let location = headers[header::LOCATION].to_str().unwrap();
    assert!(
        location.starts_with("/login?next=/api/oauth/authorize%3F"),
        "{location}"
    );

    // Signed in: back to the application with a code.
    let cookie = session_cookie(&app).await;
    let (status, headers, _) = send(
        &app,
        Request::get(authorize_uri())
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let location = headers[header::LOCATION].to_str().unwrap();
    let query = location
        .strip_prefix(&format!("{REDIRECT}?"))
        .expect("to the registered redirect URI");
    let code = query
        .split('&')
        .find_map(|p| p.strip_prefix("code="))
        .unwrap()
        .to_string();
    assert!(query.contains("state=xyz"));

    // The code, once.
    let (status, headers, body) = send(&app, token_request(&code)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let tokens = json(&body);
    let (status, _, body) = send(&app, token_request(&code)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json(&body)["error"], "invalid_grant");

    // The id_token verifies with the published key alone.
    let id_token = tokens["id_token"].as_str().unwrap();
    let (input, signature) = id_token.rsplit_once('.').unwrap();
    let (header_b64, claims_b64) = input.split_once('.').unwrap();
    let header: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header_b64).unwrap()).unwrap();
    let key = jwks["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["kid"] == header["kid"])
        .expect("the token's key is published");
    UnparsedPublicKey::new(
        &ED25519,
        URL_SAFE_NO_PAD.decode(key["x"].as_str().unwrap()).unwrap(),
    )
    .verify(
        input.as_bytes(),
        &URL_SAFE_NO_PAD.decode(signature).unwrap(),
    )
    .expect("a valid signature");
    let claims: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims_b64).unwrap()).unwrap();
    assert_eq!(claims["iss"], ISSUER);
    assert_eq!(claims["aud"], "gestion");
    assert_eq!(claims["nonce"], "n-1");
    assert_eq!(claims["email"], "ana@empresa.com");

    // And the access token reads the person back.
    let (status, _, body) = send(
        &app,
        Request::get("/oauth/userinfo")
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", tokens["access_token"].as_str().unwrap()),
            )
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body)["email"], "ana@empresa.com");
    let (status, headers, _) = send(
        &app,
        Request::get("/oauth/userinfo")
            .header(header::AUTHORIZATION, format!("Bearer {id_token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(headers[header::WWW_AUTHENTICATE]
        .to_str()
        .unwrap()
        .contains("invalid_token"));
}

#[tokio::test]
async fn an_unregistered_redirect_uri_is_refused_on_the_page_not_redirected_to() {
    let db = testkit::spawn_test_db().await;
    let app = app(&db.pool).await;
    let cookie = session_cookie(&app).await;
    let (status, headers, _) = send(
        &app,
        Request::get(
            authorize_uri().replace("gestion.example.com%2Fcallback", "evil.example.com%2Fcb"),
        )
        .header(header::COOKIE, &cookie)
        .body(Body::empty())
        .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(headers.get(header::LOCATION).is_none());
}

const SECRET: &str = "a-client-secret-long-enough-for-the-test";

/// Register a confidential client with these settings.
async fn confidential(pool: &sqlx::PgPool, id: &str, settings: auth_gateway::ClientSettings) {
    PgOidcClients::new(pool.clone())
        .insert(&OidcClient {
            id: ClientId::parse(id).unwrap(),
            name: id.into(),
            redirect_uris: vec![REDIRECT.into()],
            auth: ClientAuth::Secret {
                hash: auth_gateway::ResetTokenGenerator::hash(&ResetTokenAdapter, SECRET),
            },
            settings,
        })
        .await
        .unwrap();
}

fn basic(id: &str, secret: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{id}:{secret}"))
    )
}

/// The claims of an ES256 token, if its signature verifies with the EC key
/// of the JWK Set under its `kid` (as an ES256 relying party checks it).
fn es256_claims(jwks: &Value, token: &str) -> Value {
    let (input, signature) = token.rsplit_once('.').unwrap();
    let (header_b64, claims_b64) = input.split_once('.').unwrap();
    let header: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header_b64).unwrap()).unwrap();
    assert_eq!(header["alg"], "ES256");
    let key = jwks["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["kid"] == header["kid"])
        .expect("the token's key is published");
    assert_eq!(key["kty"], "EC");
    let mut point = vec![4u8];
    for c in ["x", "y"] {
        point.extend(URL_SAFE_NO_PAD.decode(key[c].as_str().unwrap()).unwrap());
    }
    UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, point)
        .verify(
            input.as_bytes(),
            &URL_SAFE_NO_PAD.decode(signature).unwrap(),
        )
        .expect("a valid ES256 signature");
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims_b64).unwrap()).unwrap()
}

async fn jwks_of(app: &axum::Router) -> Value {
    let (_, _, body) = send(
        app,
        Request::get("/oauth/jwks").body(Body::empty()).unwrap(),
    )
    .await;
    json(&body)
}

fn authorize_without_pkce(client: &str) -> String {
    format!(
        "/oauth/authorize?response_type=code&client_id={client}\
         &redirect_uri=https%3A%2F%2Fgestion.example.com%2Fcallback\
         &scope=openid&state=xyz&nonce=n-2"
    )
}

#[tokio::test]
async fn an_es256_client_without_pkce_signs_in_and_checks_its_id_token() {
    let db = testkit::spawn_test_db().await;
    let app = app(&db.pool).await;
    confidential(
        &db.pool,
        "sql-query",
        auth_gateway::ClientSettings {
            token_alg: auth_gateway::SigningAlg::ES256,
            pkce_required: false,
            required_policy: Some("quality-admin".into()),
            ..auth_gateway::ClientSettings::default()
        },
    )
    .await;
    let jwks = jwks_of(&app).await;
    assert_eq!(jwks["keys"].as_array().unwrap().len(), 2);

    // No code_challenge: still to sign in, not an error.
    let (status, headers, _) = send(
        &app,
        Request::get(authorize_without_pkce("sql-query"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert!(headers[header::LOCATION]
        .to_str()
        .unwrap()
        .starts_with("/login?next="));

    let cookie = session_cookie(&app).await;
    let (status, headers, _) = send(
        &app,
        Request::get(authorize_without_pkce("sql-query"))
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let location = headers[header::LOCATION].to_str().unwrap();
    let code = location
        .split(['?', '&'])
        .find_map(|p| p.strip_prefix("code="))
        .unwrap_or_else(|| panic!("a code: {location}"))
        .to_string();

    // Redeemed with the secret in Basic and no verifier.
    let (status, _, body) = send(
        &app,
        Request::post("/oauth/token")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(header::AUTHORIZATION, basic("sql-query", SECRET))
            .body(Body::from(format!(
                "grant_type=authorization_code&code={code}\
                 &redirect_uri=https%3A%2F%2Fgestion.example.com%2Fcallback"
            )))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let tokens = json(&body);
    let id = es256_claims(&jwks, tokens["id_token"].as_str().unwrap());
    assert_eq!(id["aud"], "sql-query");
    assert_eq!(id["nonce"], "n-2");
    assert_eq!(id["policy"][0], "quality-admin");
    let access = es256_claims(&jwks, tokens["access_token"].as_str().unwrap());
    assert_eq!(access["iss"], ISSUER);
}

#[tokio::test]
async fn an_account_without_the_required_policy_goes_back_with_access_denied() {
    let db = testkit::spawn_test_db().await;
    let app = app(&db.pool).await;
    confidential(
        &db.pool,
        "auditing",
        auth_gateway::ClientSettings {
            pkce_required: false,
            required_policy: Some("auditor".into()),
            ..auth_gateway::ClientSettings::default()
        },
    )
    .await;
    let cookie = session_cookie(&app).await;
    let (status, headers, _) = send(
        &app,
        Request::get(authorize_without_pkce("auditing"))
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let location = headers[header::LOCATION].to_str().unwrap();
    assert!(
        location.starts_with(&format!("{REDIRECT}?error=access_denied&state=xyz")),
        "{location}"
    );
}

fn client_credentials(auth: &str, extra: &str) -> Request<Body> {
    Request::post("/oauth/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(header::AUTHORIZATION, auth)
        .body(Body::from(format!("grant_type=client_credentials{extra}")))
        .unwrap()
}

#[tokio::test]
async fn a_machine_gets_a_short_es256_token_for_an_audience_as_its_service_account() {
    let db = testkit::spawn_test_db().await;
    let app = app(&db.pool).await;
    // A service account of Ana's with a policy attached.
    let account: String = sqlx::query_scalar(
        "INSERT INTO service_accounts (owner_id, name, access_key, secret_hash) \
         SELECT id, 'worker', 'AKTEST', 'x' FROM admin_users RETURNING id::text",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO iam_attachments (principal_kind, principal_id, policy) \
         VALUES ('service_account', $1::uuid, 'lake-reader')",
    )
    .bind(&account)
    .execute(&db.pool)
    .await
    .unwrap();
    confidential(
        &db.pool,
        "qd-worker",
        auth_gateway::ClientSettings {
            token_alg: auth_gateway::SigningAlg::ES256,
            client_credentials: true,
            audiences: vec!["sql-query".into(), "lake".into()],
            service_account: Some(account.clone()),
            ..auth_gateway::ClientSettings::default()
        },
    )
    .await;
    confidential(
        &db.pool,
        "gestion-web",
        auth_gateway::ClientSettings::default(),
    )
    .await;
    let jwks = jwks_of(&app).await;

    let (status, headers, body) =
        send(&app, client_credentials(&basic("qd-worker", SECRET), "")).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let tokens = json(&body);
    assert!(tokens.get("id_token").is_none());
    assert!(tokens.get("refresh_token").is_none());
    assert_eq!(tokens["expires_in"], 86_400);
    let claims = es256_claims(&jwks, tokens["access_token"].as_str().unwrap());
    assert_eq!(claims["aud"], "sql-query");
    assert_eq!(claims["sub"], account.as_str());
    assert_eq!(claims["client_id"], "qd-worker");
    assert_eq!(claims["policy"], serde_json::json!(["lake-reader"]));

    // Another audience of its own, by RFC 8707's `resource`.
    let (_, _, body) = send(
        &app,
        client_credentials(&basic("qd-worker", SECRET), "&resource=lake"),
    )
    .await;
    let claims = es256_claims(&jwks, json(&body)["access_token"].as_str().unwrap());
    assert_eq!(claims["aud"], "lake");

    for (auth, extra, status, error) in [
        (
            basic("qd-worker", "wrong"),
            "",
            StatusCode::UNAUTHORIZED,
            "invalid_client",
        ),
        (
            basic("qd-worker", SECRET),
            "&audience=billing",
            StatusCode::BAD_REQUEST,
            "invalid_target",
        ),
        (
            basic("gestion-web", SECRET),
            "",
            StatusCode::BAD_REQUEST,
            "unauthorized_client",
        ),
    ] {
        let (got, _, body) = send(&app, client_credentials(&auth, extra)).await;
        assert_eq!(got, status);
        assert_eq!(json(&body)["error"], error);
    }
}
