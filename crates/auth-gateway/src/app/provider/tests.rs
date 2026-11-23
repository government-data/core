use super::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use crate::app::oauth::testing::{FixedClock, FixedSecrets, InMemoryAdmins};
use crate::domain::{ClientAuth, ClientSettings};
use crate::domain::{
    CsrfToken, LockoutState, PasswordHash, RepositoryError, ResetTokenGenerator, Role,
};
use crate::AdminAccount;

const ISSUER: &str = "https://orq.example.com/api";
const REDIRECT: &str = "https://gestion.example.com/callback";
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

#[derive(Default)]
struct Clients(Mutex<HashMap<String, OidcClient>>);
#[async_trait::async_trait]
impl OidcClientRegistry for Clients {
    async fn find(&self, id: &ClientId) -> Result<Option<OidcClient>, RepositoryError> {
        Ok(self.0.lock().unwrap().get(id.as_str()).cloned())
    }
    async fn insert(&self, c: &OidcClient) -> Result<bool, RepositoryError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .insert(c.id.to_string(), c.clone())
            .is_none())
    }
    async fn upsert(&self, c: &OidcClient) -> Result<(), RepositoryError> {
        self.0.lock().unwrap().insert(c.id.to_string(), c.clone());
        Ok(())
    }
}

#[derive(Default)]
struct Codes(Mutex<HashMap<String, AuthorizationCode>>);
#[async_trait::async_trait]
impl AuthorizationCodeStore for Codes {
    async fn insert(&self, h: &str, c: &AuthorizationCode) -> Result<(), RepositoryError> {
        self.0.lock().unwrap().insert(h.to_string(), c.clone());
        Ok(())
    }
    async fn take(&self, h: &str) -> Result<Option<AuthorizationCode>, RepositoryError> {
        Ok(self.0.lock().unwrap().remove(h))
    }
}

/// Codes `code-1`, `code-2`…; the "hash" is a prefix, enough to tell
/// stored from presented.
#[derive(Default)]
struct Counter(AtomicU32);
impl ResetTokenGenerator for Counter {
    fn generate(&self) -> String {
        format!("code-{}", self.0.fetch_add(1, Ordering::SeqCst) + 1)
    }
    fn hash(&self, token: &str) -> String {
        format!("h:{token}")
    }
}

/// Signs as `typ|claims`, so a test can read what was signed.
struct PlainSigner;
impl TokenSigner for PlainSigner {
    fn sign(&self, typ: &str, claims: &str) -> String {
        format!("{typ}|{claims}")
    }
    fn verify(&self, typ: &str, token: &str) -> Option<String> {
        token.strip_prefix(&format!("{typ}|")).map(str::to_string)
    }
    fn algorithm(&self) -> &'static str {
        "EdDSA"
    }
    fn jwks(&self) -> String {
        r#"{"keys":[]}"#.into()
    }
}

struct Fixture {
    server: AuthorizationServer,
    session: AuthenticatedSession,
}

fn fixture(auth: ClientAuth) -> Fixture {
    fixture_with(auth, ClientSettings::default())
}

fn fixture_with(auth: ClientAuth, settings: ClientSettings) -> Fixture {
    let clients = Arc::new(Clients::default());
    clients.0.lock().unwrap().insert(
        "gestion".into(),
        OidcClient {
            id: ClientId::parse("gestion").unwrap(),
            name: "Gestión".into(),
            redirect_uris: vec![REDIRECT.into()],
            auth,
            settings,
        },
    );
    let admins = Arc::new(InMemoryAdmins::default());
    admins.accounts.lock().unwrap().push(AdminAccount {
        id: AdminId::new("ana"),
        email: crate::domain::Email::parse("ana@empresa.com").unwrap(),
        password_hash: PasswordHash::from_encoded("x"),
        lockout: LockoutState::clear(),
        role: Role::admin(),
        display_name: Some("Ana".into()),
    });
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let server = AuthorizationServer::new(
        clients,
        Arc::new(Codes::default()),
        Arc::new(Counter::default()),
        Arc::new(FixedSecrets),
        Arc::new(PlainSigner),
        admins,
        Arc::new(FixedClock(now)),
        format!("{ISSUER}/"),
    );
    let session = AuthenticatedSession {
        admin_id: AdminId::new("ana"),
        role: Role::admin(),
        csrf_token: CsrfToken::from_raw("csrf"),
        signed_in_at: now - Duration::from_secs(30),
    };
    Fixture { server, session }
}

fn request() -> AuthorizeRequest {
    AuthorizeRequest {
        response_type: Some("code".into()),
        client_id: Some("gestion".into()),
        redirect_uri: Some(REDIRECT.into()),
        scope: Some("openid email profile".into()),
        state: Some("st 1".into()),
        code_challenge: Some(FixedSecrets.challenge(VERIFIER)),
        code_challenge_method: Some("S256".into()),
        nonce: Some("n-1".into()),
        prompt: None,
    }
}

fn token_request(code: &str) -> TokenRequest {
    TokenRequest {
        grant_type: Some("authorization_code".into()),
        code: Some(code.into()),
        redirect_uri: Some(REDIRECT.into()),
        client_id: Some("gestion".into()),
        client_secret: None,
        code_verifier: Some(VERIFIER.into()),
        ..TokenRequest::default()
    }
}

fn claims(token: &str) -> Value {
    serde_json::from_str(token.split_once('|').unwrap().1).unwrap()
}

#[tokio::test]
async fn a_signed_in_person_gets_a_code_and_redeems_it_once() {
    let f = fixture(ClientAuth::Public);
    let outcome = f.server.authorize(&request(), Some(&f.session)).await;
    assert_eq!(
        outcome,
        AuthorizeOutcome::Redirect(format!(
            "{REDIRECT}?code=code-1&state=st%201&iss=https%3A%2F%2Forq.example.com%2Fapi"
        ))
    );

    let tokens = f.server.token(&token_request("code-1")).await.unwrap();
    let id = claims(tokens.id_token.as_deref().unwrap());
    assert_eq!(id["iss"], ISSUER);
    assert_eq!(id["sub"], "ana");
    assert_eq!(id["aud"], "gestion");
    assert_eq!(id["nonce"], "n-1");
    assert_eq!(id["email"], "ana@empresa.com");
    assert_eq!(id["name"], "Ana");
    assert_eq!(id["role"], "admin");
    assert_eq!(id["auth_time"], 1_000_000 - 30);
    assert_eq!(id["exp"], 1_000_000 + TOKEN_TTL.as_secs());
    assert!(tokens.access_token.starts_with("at+jwt|"));
    assert_eq!(tokens.scope, "openid email profile");

    // The code is spent.
    assert_eq!(
        f.server.token(&token_request("code-1")).await,
        Err(TokenError::InvalidGrant)
    );
}

#[tokio::test]
async fn without_a_session_it_asks_to_sign_in_or_says_so() {
    let f = fixture(ClientAuth::Public);
    assert_eq!(
        f.server.authorize(&request(), None).await,
        AuthorizeOutcome::SignIn
    );
    let silent = AuthorizeRequest {
        prompt: Some("none".into()),
        ..request()
    };
    assert_eq!(
        f.server.authorize(&silent, None).await,
        AuthorizeOutcome::Redirect(format!(
            "{REDIRECT}?error=login_required&state=st%201&iss=https%3A%2F%2Forq.example.com%2Fapi"
        ))
    );
}

#[tokio::test]
async fn an_unknown_client_or_redirect_uri_is_never_redirected_to() {
    let f = fixture(ClientAuth::Public);
    let other_client = AuthorizeRequest {
        client_id: Some("otra".into()),
        ..request()
    };
    assert!(matches!(
        f.server.authorize(&other_client, Some(&f.session)).await,
        AuthorizeOutcome::Invalid(_)
    ));
    let other_uri = AuthorizeRequest {
        redirect_uri: Some("https://evil.example.com/callback".into()),
        ..request()
    };
    assert!(matches!(
        f.server.authorize(&other_uri, Some(&f.session)).await,
        AuthorizeOutcome::Invalid(_)
    ));
}

#[tokio::test]
async fn pkce_and_openid_are_required() {
    let f = fixture(ClientAuth::Public);
    for (req, error) in [
        (
            AuthorizeRequest {
                code_challenge: None,
                ..request()
            },
            "invalid_request",
        ),
        (
            AuthorizeRequest {
                code_challenge_method: Some("plain".into()),
                ..request()
            },
            "invalid_request",
        ),
        (
            AuthorizeRequest {
                scope: Some("email".into()),
                ..request()
            },
            "invalid_scope",
        ),
        (
            AuthorizeRequest {
                response_type: Some("token".into()),
                ..request()
            },
            "unsupported_response_type",
        ),
    ] {
        let AuthorizeOutcome::Redirect(to) = f.server.authorize(&req, Some(&f.session)).await
        else {
            panic!("expected a redirect for {error}");
        };
        assert!(
            to.starts_with(&format!("{REDIRECT}?error={error}&")),
            "{to}"
        );
    }
}

#[tokio::test]
async fn a_wrong_verifier_redirect_or_client_spends_the_code() {
    let f = fixture(ClientAuth::Public);
    f.server.authorize(&request(), Some(&f.session)).await;
    let wrong = TokenRequest {
        code_verifier: Some("x".repeat(43)),
        ..token_request("code-1")
    };
    assert_eq!(f.server.token(&wrong).await, Err(TokenError::InvalidGrant));
    // Spent even though the right verifier comes next.
    assert_eq!(
        f.server.token(&token_request("code-1")).await,
        Err(TokenError::InvalidGrant)
    );

    f.server.authorize(&request(), Some(&f.session)).await;
    let other_uri = TokenRequest {
        redirect_uri: Some("https://gestion.example.com/otra".into()),
        ..token_request("code-2")
    };
    assert_eq!(
        f.server.token(&other_uri).await,
        Err(TokenError::InvalidGrant)
    );
}

#[tokio::test]
async fn a_confidential_client_must_bring_its_secret() {
    let f = fixture(ClientAuth::Secret {
        hash: "h:s3cret".into(),
    });
    f.server.authorize(&request(), Some(&f.session)).await;
    assert_eq!(
        f.server.token(&token_request("code-1")).await,
        Err(TokenError::InvalidClient)
    );
    let wrong = TokenRequest {
        client_secret: Some("otro".into()),
        ..token_request("code-1")
    };
    assert_eq!(f.server.token(&wrong).await, Err(TokenError::InvalidClient));
    let right = TokenRequest {
        client_secret: Some("s3cret".into()),
        ..token_request("code-1")
    };
    assert!(f.server.token(&right).await.is_ok());
}

#[tokio::test]
async fn userinfo_reads_a_live_access_token() {
    let f = fixture(ClientAuth::Public);
    f.server.authorize(&request(), Some(&f.session)).await;
    let tokens = f.server.token(&token_request("code-1")).await.unwrap();
    let info = f.server.userinfo(&tokens.access_token).await.unwrap();
    assert_eq!(info["sub"], "ana");
    assert_eq!(info["email"], "ana@empresa.com");
    // An id_token is not an access token.
    assert!(f
        .server
        .userinfo(tokens.id_token.as_deref().unwrap())
        .await
        .is_none());
    assert!(f.server.userinfo("at+jwt|{}").await.is_none());
}

#[test]
fn discovery_points_under_the_issuer() {
    let f = fixture(ClientAuth::Public);
    let d = f.server.discovery();
    assert_eq!(d["issuer"], ISSUER);
    assert_eq!(d["token_endpoint"], format!("{ISSUER}/oauth/token"));
    assert_eq!(d["code_challenge_methods_supported"], json!(["S256"]));
}

/// The `policy` claim's source, fixed (or failing).
struct Policies(Result<Vec<String>, ()>);

#[async_trait::async_trait]
impl crate::domain::AccountPolicies for Policies {
    async fn policies(
        &self,
        _admin: &crate::domain::AdminAccount,
    ) -> Result<Vec<String>, crate::domain::RepositoryError> {
        self.0
            .clone()
            .map_err(|()| crate::domain::RepositoryError::Backend("down".into()))
    }
}

#[tokio::test]
async fn the_tokens_carry_the_policy_names_for_the_applications() {
    // Without a source, no claim (as before ADR 0016).
    let f = fixture(ClientAuth::Public);
    let _ = f.server.authorize(&request(), Some(&f.session)).await;
    let tokens = f.server.token(&token_request("code-1")).await.unwrap();
    assert!(claims(tokens.id_token.as_deref().unwrap())
        .get("policy")
        .is_none());

    // With one, in the id_token, the access token and userinfo.
    let mut f = fixture(ClientAuth::Public);
    f.server = f.server.with_policies(Arc::new(Policies(Ok(vec![
        "quality-admin".into(),
        "auditor".into(),
    ]))));
    let _ = f.server.authorize(&request(), Some(&f.session)).await;
    let tokens = f.server.token(&token_request("code-1")).await.unwrap();
    assert_eq!(
        claims(tokens.id_token.as_deref().unwrap())["policy"],
        serde_json::json!(["quality-admin", "auditor"])
    );
    assert_eq!(
        claims(&tokens.access_token)["policy"],
        serde_json::json!(["quality-admin", "auditor"])
    );
    let info = f.server.userinfo(&tokens.access_token).await.unwrap();
    assert_eq!(
        info["policy"],
        serde_json::json!(["quality-admin", "auditor"])
    );

    // If they cannot be read, an empty list: nothing is granted.
    let mut f = fixture(ClientAuth::Public);
    f.server = f.server.with_policies(Arc::new(Policies(Err(()))));
    let _ = f.server.authorize(&request(), Some(&f.session)).await;
    let tokens = f.server.token(&token_request("code-1")).await.unwrap();
    assert_eq!(
        claims(tokens.id_token.as_deref().unwrap())["policy"],
        serde_json::json!([])
    );
}

mod certificate;
mod job_token;
mod settings;

#[tokio::test]
async fn the_installations_own_machine_client_gets_its_token_without_a_secret() {
    let machine = ClientSettings {
        client_credentials: true,
        audiences: vec!["sql-query".into()],
        ..ClientSettings::default()
    };
    let secret = ClientAuth::Secret { hash: "h:s".into() };
    let f = fixture_with(secret, machine.clone());
    let token = f
        .server
        .own_machine_token("gestion", "sql-query")
        .await
        .unwrap();
    let c = claims(&token.access_token);
    assert_eq!(
        (c["aud"].as_str(), c["sub"].as_str()),
        (Some("sql-query"), Some("gestion"))
    );
    assert_eq!(c["client_id"], "gestion");

    // Another audience, an unknown client, or one that is no machine: none.
    let other = f.server.own_machine_token("gestion", "lake").await;
    assert_eq!(other.unwrap_err().code(), "invalid_target");
    let unknown = f.server.own_machine_token("nadie", "sql-query").await;
    assert_eq!(unknown.unwrap_err().code(), "invalid_client");
    let person = fixture(ClientAuth::Secret { hash: "h:s".into() });
    let refused = person
        .server
        .own_machine_token("gestion", "sql-query")
        .await;
    assert_eq!(refused.unwrap_err().code(), "unauthorized_client");
}
