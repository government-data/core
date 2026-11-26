//! The main lake's users with given keys (decision 0091, ruling 51): the
//! query engine's keys read from its role's secrets, and each user asked of
//! the main lake's adapter with them.

use std::collections::BTreeMap;

use crate::lake::{LakeCredentialKind, LakeError, LakeKeys};
use crate::machines::{query_engine_keys, QUERY_ENGINE_ACCESS_KEY, QUERY_ENGINE_SECRET_KEY};

use super::lake_adapters::Asked;
use super::lake_replication::run;
use super::*;

fn keys(access: &str) -> LakeKeys {
    LakeKeys {
        access_key: access.into(),
        secret_key: "a-secret-of-forty-characters-0123456789".into(),
    }
}

#[test]
fn the_query_engine_s_keys_are_both_role_secrets() {
    let both: BTreeMap<String, String> = [
        (
            QUERY_ENGINE_ACCESS_KEY.to_string(),
            "trino-ak\n".to_string(),
        ),
        (
            QUERY_ENGINE_SECRET_KEY.to_string(),
            "the-secret".to_string(),
        ),
        ("other".to_string(), "x".to_string()),
    ]
    .into();
    let found = query_engine_keys(&both).unwrap();
    assert_eq!(
        (found.access_key.as_str(), found.secret_key.as_str()),
        ("trino-ak", "the-secret")
    );
    let mut one = both.clone();
    one.remove(QUERY_ENGINE_SECRET_KEY);
    assert_eq!(query_engine_keys(&one), None);
    let mut empty = both;
    empty.insert(QUERY_ENGINE_ACCESS_KEY.into(), " ".into());
    assert_eq!(query_engine_keys(&empty), None);
}

#[tokio::test]
async fn a_user_with_given_keys_is_asked_of_the_main_lake() {
    let r = run(None, Vec::new());
    let s = &r.w.service;
    s.ensure_lake_user(LakeCredentialKind::QueryEngine, &keys("trino-ak"))
        .await
        .unwrap();
    s.ensure_lake_user(LakeCredentialKind::StsIssuer, &keys("sts-ak"))
        .await
        .unwrap();
    let refused = s
        .ensure_lake_user(LakeCredentialKind::Admin, &keys("x"))
        .await;
    assert!(
        matches!(refused, Err(LakeError::Invalid(_))),
        "the lake makes the admin's"
    );
    let main = auth_gateway::Lake::main().key();
    assert_eq!(
        *r.lakes.asked.lock().unwrap(),
        [
            Asked::Credentials(
                main.clone(),
                LakeCredentialKind::QueryEngine,
                Some("trino-ak".into())
            ),
            Asked::Credentials(main, LakeCredentialKind::StsIssuer, Some("sts-ak".into())),
        ]
    );
    let without = world().service;
    let unreached = without
        .ensure_lake_user(LakeCredentialKind::QueryEngine, &keys("trino-ak"))
        .await;
    assert!(matches!(unreached, Err(LakeError::Unavailable(_))));
}
