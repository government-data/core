//! A job's token for a capability's audience (decision 0062): signed as the installation's
//! own machine client, naming the job and the machine it goes to, for 15
//! minutes whatever the provider's other tokens last.

use super::*;

const SUBJECT: &str = "job:00000000-0000-0000-0000-00000000000a:\
                       7f0c2a52-1c1e-4c43-9a55-0d2f1e0b6a10:read:src_core";

fn machine() -> ClientSettings {
    ClientSettings {
        client_credentials: true,
        audiences: vec!["sql-query".into()],
        ..ClientSettings::default()
    }
}

fn secret() -> ClientAuth {
    ClientAuth::Secret { hash: "h:s".into() }
}

#[tokio::test]
async fn a_jobs_token_names_the_job_and_its_machine_for_fifteen_minutes() {
    let f = fixture_with(secret(), machine());
    let token = f
        .server
        .job_token("gestion", "sql-query", SUBJECT, "qd-worker-1")
        .await
        .unwrap();
    assert_eq!(token.expires_in, 15 * 60);
    assert!(token.id_token.is_none());
    assert!(token.access_token.starts_with("at+jwt|"));
    let c = claims(&token.access_token);
    assert_eq!(c["iss"], ISSUER);
    assert_eq!(c["aud"], "sql-query");
    assert_eq!(c["sub"], SUBJECT);
    assert_eq!(c["act"], serde_json::json!({ "sub": "qd-worker-1" }));
    assert_eq!(c["iat"], 1_000_000);
    assert_eq!(c["exp"], 1_000_000 + 15 * 60);
    assert!(c["jti"].is_string());
    // Not a machine's token: no client, no policy.
    assert!(c.get("client_id").is_none() && c.get("policy").is_none());
}

#[tokio::test]
async fn a_jobs_token_needs_the_installations_own_client_allowed_the_audience() {
    let f = fixture_with(secret(), machine());
    let other = f.server.job_token("gestion", "lake", SUBJECT, "w").await;
    assert_eq!(other.unwrap_err().code(), "invalid_target");
    let unknown = f.server.job_token("nadie", "sql-query", SUBJECT, "w").await;
    assert_eq!(unknown.unwrap_err().code(), "invalid_client");
    let person = fixture(secret());
    let refused = person
        .server
        .job_token("gestion", "sql-query", SUBJECT, "w")
        .await;
    assert_eq!(refused.unwrap_err().code(), "unauthorized_client");
}

#[tokio::test]
async fn a_jobs_token_is_no_machine_token_for_the_api() {
    let both = ClientSettings {
        audiences: vec!["sql-query".into(), "quality-api".into()],
        ..machine()
    };
    let f = fixture_with(secret(), both);
    let token = f
        .server
        .job_token("gestion", "quality-api", SUBJECT, "w")
        .await
        .unwrap();
    assert_eq!(
        f.server.machine_client(&token.access_token, "quality-api"),
        None
    );
}
