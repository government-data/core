//! A module's client (quality-sdk decision 0009): disabled, it gets no code
//! and no token, and its tokens speak for no one at `/userinfo`; its
//! previous secret is taken until its time; its role claim carries the
//! values whose action the person holds.

use super::*;
use crate::domain::{ModuleClient, PreviousSecret, RoleClaim, RoleValue};

const SECRET: &str = "s3cret";

fn module_settings(change: impl FnOnce(&mut ModuleClient)) -> ClientSettings {
    let mut module = ModuleClient::of("bi-superset");
    change(&mut module);
    ClientSettings {
        module: Some(module),
        ..ClientSettings::default()
    }
}

fn secret() -> ClientAuth {
    ClientAuth::Secret {
        hash: format!("h:{SECRET}"),
    }
}

fn with_secret(code: &str, secret: &str) -> TokenRequest {
    TokenRequest {
        client_secret: Some(secret.into()),
        ..token_request(code)
    }
}

/// Holds `quality:ViewResults` only.
struct Viewer;
#[async_trait::async_trait]
impl ActionHolder for Viewer {
    async fn held(
        &self,
        _: &AdminAccount,
        actions: &[String],
    ) -> Result<Vec<String>, RepositoryError> {
        Ok(actions
            .iter()
            .filter(|a| *a == "quality:ViewResults")
            .cloned()
            .collect())
    }
}

#[tokio::test]
async fn a_disabled_module_client_gets_no_code_and_no_token() {
    let f = fixture_with(secret(), module_settings(|_| {}));
    assert!(matches!(
        f.server.authorize(&request(), Some(&f.session)).await,
        AuthorizeOutcome::Redirect(_)
    ));
    let tokens = f
        .server
        .token(&with_secret("code-1", SECRET))
        .await
        .unwrap();
    assert!(f.server.userinfo(&tokens.access_token).await.is_some());

    let disabled = fixture_with(secret(), module_settings(|m| m.disabled = true));
    assert!(matches!(
        disabled
            .server
            .authorize(&request(), Some(&disabled.session))
            .await,
        AuthorizeOutcome::Invalid(_)
    ));
    assert_eq!(
        disabled.server.token(&with_secret("code-1", SECRET)).await,
        Err(TokenError::InvalidClient)
    );
    assert!(
        disabled
            .server
            .userinfo(&tokens.access_token)
            .await
            .is_none(),
        "a token issued before it was disabled"
    );
}

#[tokio::test]
async fn the_previous_secret_is_taken_until_its_time() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    for (until, taken) in [(now + Duration::from_secs(60), true), (now, false)] {
        let settings = module_settings(|m| {
            m.previous_secret = Some(PreviousSecret {
                hash: "h:old".into(),
                until,
            });
        });
        let f = fixture_with(secret(), settings);
        f.server.authorize(&request(), Some(&f.session)).await;
        let result = f.server.token(&with_secret("code-1", "old")).await;
        assert_eq!(result.is_ok(), taken, "until {until:?}");
    }
}

#[tokio::test]
async fn the_role_claim_has_the_values_of_the_actions_held() {
    let claim = RoleClaim {
        name: "bi_roles".into(),
        values: vec![
            RoleValue {
                value: "viewer".into(),
                action: "quality:ViewResults".into(),
            },
            RoleValue {
                value: "author".into(),
                action: "bi-superset:Author".into(),
            },
        ],
    };
    let settings = module_settings(|m| m.role_claim = Some(claim));
    let f = fixture_with(secret(), settings.clone());
    let server = f.server.clone().with_actions(Arc::new(Viewer));
    server.authorize(&request(), Some(&f.session)).await;
    let tokens = server.token(&with_secret("code-1", SECRET)).await.unwrap();
    let id = claims(tokens.id_token.as_deref().unwrap());
    assert_eq!(id["bi_roles"], json!(["viewer"]));
    let info = server.userinfo(&tokens.access_token).await.unwrap();
    assert_eq!(info["bi_roles"], json!(["viewer"]));

    // Without a source of actions, the claim is there and empty.
    let f = fixture_with(secret(), settings);
    f.server.authorize(&request(), Some(&f.session)).await;
    let tokens = f
        .server
        .token(&with_secret("code-1", SECRET))
        .await
        .unwrap();
    let id = claims(tokens.id_token.as_deref().unwrap());
    assert_eq!(id["bi_roles"], json!([]));
}
