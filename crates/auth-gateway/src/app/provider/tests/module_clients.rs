//! A module's clients as the update agent registers them (quality-sdk
//! decision 0009): registered once, the same request again changes nothing,
//! a rotation keeps the previous secret, a client no longer named is
//! disabled, a client registered by hand is never taken over, and what is
//! sent is checked before anything is stored.

use super::*;

const ORIGIN: &str = "https://qd.example.com";
const PEOPLE: &str = "people-secret-0123456789abcdef";
const MACHINE: &str = "machine-secret-0123456789abcde";

fn body(people_secret: &str, keep_previous: bool, machine: bool) -> ModuleClientsRequest {
    let json = json!({
        "people": {
            "redirect_paths": ["/bi/oauth-authorized/qd"],
            "logout_paths": ["/bi/"],
            "alg": "ES256",
            "role_claim": { "name": "bi_roles", "values": { "author": "bi-superset:Author" } },
            "secret": people_secret,
            "keep_previous": keep_previous,
        },
        "machine": machine.then(|| json!({
            "capabilities": ["sql-query"], "audiences": ["trino"], "secret": MACHINE,
        })),
    });
    serde_json::from_value(json).unwrap()
}

fn machine_token(secret: &str) -> TokenRequest {
    TokenRequest {
        grant_type: Some("client_credentials".into()),
        client_id: Some("bi-superset-machine".into()),
        client_secret: Some(secret.into()),
        ..TokenRequest::default()
    }
}

#[tokio::test]
async fn a_modules_clients_are_registered_once_and_disabled_when_not_named() {
    let f = fixture(ClientAuth::Public);
    let done = f
        .server
        .put_module_clients("bi-superset", &body(PEOPLE, false, true), ORIGIN)
        .await
        .unwrap();
    assert_eq!(done.issuer, ISSUER);
    assert_eq!(done.people_client_id.as_deref(), Some("bi-superset"));
    assert_eq!(
        done.machine_client_id.as_deref(),
        Some("bi-superset-machine")
    );
    assert_eq!(
        done.changes,
        [
            ClientChange::Registered("bi-superset".into()),
            ClientChange::Registered("bi-superset-machine".into()),
        ]
    );
    let again = f
        .server
        .put_module_clients("bi-superset", &body(PEOPLE, false, true), ORIGIN)
        .await
        .unwrap();
    assert!(again.changes.is_empty(), "twice is once");

    let token = f.server.token(&machine_token(MACHINE)).await.unwrap();
    assert_eq!(claims(&token.access_token)["aud"], "trino");

    // The machine client no longer named: disabled.
    let people_only = f
        .server
        .put_module_clients("bi-superset", &body(PEOPLE, false, false), ORIGIN)
        .await
        .unwrap();
    assert_eq!(
        people_only.changes,
        [ClientChange::Disabled("bi-superset-machine".into())]
    );
    assert_eq!(
        f.server.token(&machine_token(MACHINE)).await,
        Err(TokenError::InvalidClient)
    );
    let removed = f
        .server
        .disable_module_clients("bi-superset")
        .await
        .unwrap();
    assert_eq!(removed, [ClientChange::Disabled("bi-superset".into())]);
    let deleted = f.server.delete_module_clients("bi-superset").await.unwrap();
    assert_eq!(deleted.len(), 2);
}

#[tokio::test]
async fn a_rotation_keeps_the_previous_secret_for_an_hour() {
    let f = fixture(ClientAuth::Public);
    let put = |secret: &'static str, keep: bool| {
        let server = f.server.clone();
        async move {
            server
                .put_module_clients("bi-superset", &body(secret, keep, true), ORIGIN)
                .await
                .unwrap()
        }
    };
    put(PEOPLE, false).await;
    let next = "people-secret-next-0123456789ab";
    let rotated = put(next, true).await;
    assert_eq!(
        rotated.changes,
        [ClientChange::Changed("bi-superset".into())]
    );
    assert!(put(next, true).await.changes.is_empty(), "twice is once");
    let client = |s: &AuthorizationServer| {
        let s = s.clone();
        async move {
            s.clients
                .find(&ClientId::parse("bi-superset").unwrap())
                .await
                .unwrap()
                .unwrap()
        }
    };
    let kept = client(&f.server).await;
    let previous = kept.settings.module.unwrap().previous_secret.unwrap();
    assert_eq!(previous.hash, format!("h:{PEOPLE}"));
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    assert_eq!(previous.until, now + PREVIOUS_SECRET_TTL);
    put(next, false).await;
    let done = client(&f.server).await;
    assert!(done.settings.module.unwrap().previous_secret.is_none());
}

#[tokio::test]
async fn a_hand_registered_client_or_a_bad_request_is_refused() {
    let f = fixture(ClientAuth::Public);
    let taken = f
        .server
        .put_module_clients("gestion", &body(PEOPLE, false, false), ORIGIN)
        .await;
    assert_eq!(taken, Err(ModuleClientsError::Taken("gestion".into())));
    let bad = |change: &dyn Fn(&mut Value)| {
        let mut v = json!({ "people": {
            "redirect_paths": ["/bi/cb"], "secret": PEOPLE,
            "role_claim": { "name": "bi_roles", "values": { "author": "bi-superset:Author" } } } });
        change(&mut v);
        serde_json::from_value::<ModuleClientsRequest>(v).unwrap()
    };
    for req in [
        bad(&|v| v["people"]["redirect_paths"] = json!(["https://evil.example.com/cb"])),
        bad(&|v| v["people"]["redirect_paths"] = json!(["/bi/../api"])),
        bad(&|v| v["people"]["redirect_paths"] = json!([])),
        bad(&|v| v["people"]["secret"] = json!("short")),
        bad(&|v| v["people"]["alg"] = json!("RS256")),
        bad(&|v| v["people"]["role_claim"]["name"] = json!("policy")),
        bad(&|v| v["people"]["role_claim"]["values"] = json!({ "a": "not-an-action" })),
    ] {
        let refused = f
            .server
            .put_module_clients("bi-superset", &req, ORIGIN)
            .await;
        let Err(ModuleClientsError::Invalid(why)) = refused else {
            panic!("{refused:?}");
        };
        assert!(!why.contains(PEOPLE), "never the secret");
    }
    assert!(f
        .server
        .put_module_clients("Bad", &body(PEOPLE, false, false), ORIGIN)
        .await
        .is_err());
}

#[tokio::test]
async fn a_module_clients_tokens_last_15_minutes_and_a_core_clients_a_day() {
    let f = fixture(ClientAuth::Public);
    // EdDSA, the fixture's only key.
    let eddsa: ModuleClientsRequest = serde_json::from_value(json!({
        "people": { "redirect_paths": ["/bi/oauth-authorized/qd"], "secret": PEOPLE },
        "machine": { "capabilities": ["sql-query"], "audiences": ["trino"], "secret": MACHINE },
    }))
    .unwrap();
    f.server
        .put_module_clients("bi-superset", &eddsa, ORIGIN)
        .await
        .unwrap();
    let now = 1_000_000;
    let machine = f.server.token(&machine_token(MACHINE)).await.unwrap();
    assert_eq!(machine.expires_in, MODULE_TOKEN_TTL.as_secs());
    assert_eq!(claims(&machine.access_token)["exp"], now + 15 * 60);

    let people = AuthorizeRequest {
        client_id: Some("bi-superset".into()),
        redirect_uri: Some(format!("{ORIGIN}/bi/oauth-authorized/qd")),
        ..request()
    };
    let code = code_of(f.server.authorize(&people, Some(&f.session)).await);
    let redeem = TokenRequest {
        client_id: Some("bi-superset".into()),
        client_secret: Some(PEOPLE.into()),
        redirect_uri: people.redirect_uri.clone(),
        ..token_request(&code)
    };
    let tokens = f.server.token(&redeem).await.unwrap();
    assert_eq!(tokens.expires_in, 15 * 60);
    assert_eq!(claims(&tokens.access_token)["exp"], now + 15 * 60);
    assert_eq!(
        claims(tokens.id_token.as_deref().unwrap())["exp"],
        now + 15 * 60
    );

    // The core's own client (registered by hand) keeps a day.
    let code = code_of(f.server.authorize(&request(), Some(&f.session)).await);
    let core = f.server.token(&token_request(&code)).await.unwrap();
    assert_eq!(core.expires_in, TOKEN_TTL.as_secs());
    assert_eq!(claims(&core.access_token)["exp"], now + TOKEN_TTL.as_secs());
}

/// The code an `/authorize` sent back.
fn code_of(outcome: AuthorizeOutcome) -> String {
    let AuthorizeOutcome::Redirect(to) = outcome else {
        panic!("{outcome:?}");
    };
    let code = to.split("code=").nth(1).unwrap();
    code.split('&').next().unwrap().to_string()
}
