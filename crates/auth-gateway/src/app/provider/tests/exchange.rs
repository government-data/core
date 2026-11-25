//! Token exchange (core decision 0089 step 4): a live person's token,
//! issued to the module's own people client, is exchanged by the module's
//! machine client for a Trino token that speaks for the person with the
//! module as actor; an expired one, another client's, a client not
//! consuming `sql-query`, an inactive account are refused; `exp` is never
//! past the subject's; the credential is recorded before it is given.

use domain::{AuditEvent, AuditId, NewActionAudit, NewAuditEvent};

use super::*;
use crate::app::provider::exchange::{ACCESS_TOKEN_TYPE, TOKEN_EXCHANGE};

const ORIGIN: &str = "https://qd.example.com";
const MACHINE: &str = "machine-secret-0123456789abcde";
const PEOPLE: &str = "people-secret-0123456789abcdef";
/// The fixture's clock, in Unix seconds.
const NOW: u64 = 1_000_000;

/// The records, in order; or a store that fails.
#[derive(Default)]
struct Records {
    actions: Mutex<Vec<NewActionAudit>>,
    failing: bool,
}

#[async_trait::async_trait]
impl domain::AuditRepository for Records {
    async fn record(&self, _: &NewAuditEvent) -> Result<AuditId, RepositoryError> {
        Ok(AuditId::new("a"))
    }
    async fn record_action(&self, e: &NewActionAudit) -> Result<AuditId, RepositoryError> {
        if self.failing {
            return Err(RepositoryError::Backend("down".into()));
        }
        self.actions.lock().unwrap().push(e.clone());
        Ok(AuditId::new("a"))
    }
    async fn list_recent(&self, _: u32) -> Result<Vec<AuditEvent>, RepositoryError> {
        Ok(Vec::new())
    }
}

async fn register(f: &Fixture, module: &str, capabilities: &[&str]) {
    let body: ModuleClientsRequest = serde_json::from_value(json!({
        "people": { "redirect_paths": [format!("/{module}/cb")], "secret": PEOPLE },
        "machine": { "capabilities": capabilities, "audiences": ["trino"], "secret": MACHINE },
    }))
    .unwrap();
    f.server
        .put_module_clients(module, &body, ORIGIN)
        .await
        .unwrap();
}

/// A person's access token as the provider signs it (`PlainSigner`).
fn person_token(client: &str, sub: &str, exp: u64) -> String {
    let claims = json!({
        "iss": ISSUER, "sub": sub, "aud": client, "client_id": client,
        "iat": NOW - 60, "exp": exp, "scope": "openid", "role": "admin",
    });
    format!("at+jwt|{claims}")
}

fn exchange(client: &str, subject: &str) -> TokenRequest {
    TokenRequest {
        grant_type: Some(TOKEN_EXCHANGE.into()),
        client_id: Some(format!("{client}-machine")),
        client_secret: Some(MACHINE.into()),
        audience: Some("trino".into()),
        subject_token: Some(subject.into()),
        subject_token_type: Some(ACCESS_TOKEN_TYPE.into()),
        ..TokenRequest::default()
    }
}

async fn world(records: Arc<Records>) -> Fixture {
    let mut f = fixture(ClientAuth::Public);
    f.server = f.server.with_audit(records);
    register(&f, "bi-superset", &["sql-query"]).await;
    register(&f, "grafana", &["metrics"]).await;
    f
}

#[tokio::test]
async fn a_live_persons_token_is_exchanged_for_the_person_with_the_module_as_actor() {
    let records = Arc::new(Records::default());
    let f = world(records.clone()).await;
    let subject = person_token("bi-superset", "ana", NOW + 3600);
    let t = f
        .server
        .token(&exchange("bi-superset", &subject))
        .await
        .unwrap();
    let c = claims(&t.access_token);
    assert_eq!(c["sub"], "person-ana");
    assert_eq!(c["aud"], "trino");
    assert_eq!(c["act"], json!({ "sub": "bi-superset" }));
    assert_eq!(c["iss"], ISSUER);
    assert_eq!(c["exp"], NOW + 900, "15 minutes at most");
    assert!(c.get("client_id").is_none(), "not a machine token");
    assert!(
        c.get("scope").is_none(),
        "not a person's token: no exchange of it"
    );
    assert_eq!(t.expires_in, 900);
    assert!(t.id_token.is_none());

    let recorded = records.actions.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].event_type,
        domain::AuditEventType::CredentialsIssued
    );
    assert_eq!(recorded[0].action, "token-exchange:trino");
    assert_eq!(recorded[0].resource, "module/bi-superset");
    assert_eq!(
        recorded[0].admin_id.as_ref().map(AdminId::as_str),
        Some("ana")
    );
    assert!(!format!("{recorded:?}").contains(&t.access_token));

    // The exchanged token cannot be exchanged again.
    let again = f
        .server
        .token(&exchange("bi-superset", &t.access_token))
        .await;
    assert_eq!(again.unwrap_err().code(), "invalid_grant");
}

#[tokio::test]
async fn exp_is_never_past_the_subjects() {
    let f = world(Arc::new(Records::default())).await;
    let subject = person_token("bi-superset", "ana", NOW + 120);
    let t = f
        .server
        .token(&exchange("bi-superset", &subject))
        .await
        .unwrap();
    assert_eq!(claims(&t.access_token)["exp"], NOW + 120);
    assert_eq!(t.expires_in, 120);
}

#[tokio::test]
async fn an_expired_another_clients_or_an_inactive_persons_token_is_refused() {
    let records = Arc::new(Records::default());
    let f = world(records.clone()).await;
    for (why, subject) in [
        ("expired", person_token("bi-superset", "ana", NOW)),
        (
            "another module's client",
            person_token("grafana", "ana", NOW + 3600),
        ),
        (
            "a client by hand",
            person_token("gestion", "ana", NOW + 3600),
        ),
        (
            "no such account",
            person_token("bi-superset", "nadie", NOW + 3600),
        ),
        (
            "not ours",
            format!(
                "at+jwt|{}",
                json!({ "iss": "https://other", "sub": "ana",
            "client_id": "bi-superset", "exp": NOW + 3600, "scope": "openid" })
            ),
        ),
        ("forged", "id+jwt|{}".to_string()),
    ] {
        let refused = f.server.token(&exchange("bi-superset", &subject)).await;
        assert_eq!(refused.unwrap_err().code(), "invalid_grant", "{why}");
    }
    let mut locked = AdminAccount {
        id: AdminId::new("bruno"),
        email: crate::domain::Email::parse("bruno@empresa.com").unwrap(),
        password_hash: PasswordHash::from_encoded("x"),
        lockout: LockoutState::clear(),
        role: Role::admin(),
        display_name: None,
    };
    locked.lockout.locked_until = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(NOW + 60));
    let f = locked_world(locked, records.clone()).await;
    let subject = person_token("bi-superset", "bruno", NOW + 3600);
    let refused = f.server.token(&exchange("bi-superset", &subject)).await;
    assert_eq!(refused.unwrap_err().code(), "invalid_grant", "locked out");
    assert!(
        records.actions.lock().unwrap().is_empty(),
        "nothing recorded"
    );
}

async fn locked_world(account: AdminAccount, records: Arc<Records>) -> Fixture {
    let clients = Arc::new(Clients::default());
    let admins = Arc::new(InMemoryAdmins::default());
    admins.accounts.lock().unwrap().push(account);
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(NOW);
    let server = AuthorizationServer::new(
        clients,
        Arc::new(Codes::default()),
        Arc::new(Counter::default()),
        Arc::new(FixedSecrets),
        Arc::new(PlainSigner),
        admins,
        Arc::new(FixedClock(now)),
        ISSUER,
    )
    .with_audit(records);
    let f = Fixture {
        server,
        session: fixture(ClientAuth::Public).session,
    };
    register(&f, "bi-superset", &["sql-query"]).await;
    f
}

#[tokio::test]
async fn a_client_not_consuming_sql_query_or_with_a_bad_secret_is_refused() {
    let f = world(Arc::new(Records::default())).await;
    let subject = person_token("grafana", "ana", NOW + 3600);
    let refused = f.server.token(&exchange("grafana", &subject)).await;
    assert_eq!(refused.unwrap_err().code(), "unauthorized_client");
    let subject = person_token("bi-superset", "ana", NOW + 3600);
    let bad = TokenRequest {
        client_secret: Some("wrong".into()),
        ..exchange("bi-superset", &subject)
    };
    assert_eq!(
        f.server.token(&bad).await.unwrap_err().code(),
        "invalid_client"
    );
    let other_kind = TokenRequest {
        subject_token_type: Some("urn:ietf:params:oauth:token-type:id_token".into()),
        ..exchange("bi-superset", &subject)
    };
    assert_eq!(
        f.server.token(&other_kind).await.unwrap_err().code(),
        "invalid_request"
    );
    let other_audience = TokenRequest {
        audience: Some("lake".into()),
        ..exchange("bi-superset", &subject)
    };
    assert_eq!(
        f.server.token(&other_audience).await.unwrap_err().code(),
        "invalid_target"
    );
}

#[tokio::test]
async fn without_a_record_no_token_is_given() {
    let failing = Arc::new(Records {
        failing: true,
        ..Records::default()
    });
    let f = world(failing).await;
    let subject = person_token("bi-superset", "ana", NOW + 3600);
    let refused = f.server.token(&exchange("bi-superset", &subject)).await;
    assert_eq!(refused.unwrap_err().code(), "server_error");
    let unrecorded = fixture(ClientAuth::Public);
    register(&unrecorded, "bi-superset", &["sql-query"]).await;
    let refused = unrecorded
        .server
        .token(&exchange("bi-superset", &subject))
        .await;
    assert_eq!(
        refused.unwrap_err().code(),
        "server_error",
        "no audit configured"
    );
}
