//! A module's clients on Postgres (`oidc_clients`, quality-sdk decision
//! 0009), connected as the application's role: saved whole and read back
//! with their module side, replaced in place, disabled and enabled again,
//! listed and deleted by module; a client registered by hand, or another
//! module's, is never taken over. Needs Docker.

use std::time::{Duration, SystemTime};

use auth_gateway::{
    ClientAuth, ClientId, ClientSettings, ModuleClient, OidcClient, OidcClientRegistry,
    PreviousSecret, RoleClaim, RoleValue, SigningAlg,
};
use infrastructure::oidc::PgOidcClients;
use sqlx::postgres::PgPoolOptions;

fn people(module: &str) -> OidcClient {
    let mut side = ModuleClient::of(module);
    side.role_claim = Some(RoleClaim {
        name: "bi_roles".into(),
        values: vec![RoleValue {
            value: "author".into(),
            action: "bi-superset:Author".into(),
        }],
    });
    side.logout_uris = vec!["https://qd.example.com/bi/".into()];
    side.requires = vec!["sql-query".into()];
    OidcClient {
        id: ClientId::parse(module).unwrap(),
        name: module.into(),
        redirect_uris: vec!["https://qd.example.com/bi/oauth-authorized/qd".into()],
        auth: ClientAuth::Secret {
            hash: "hash-1".into(),
        },
        settings: ClientSettings {
            token_alg: SigningAlg::ES256,
            module: Some(side),
            ..ClientSettings::default()
        },
    }
}

#[tokio::test]
async fn a_modules_clients_are_kept_whole_and_never_take_over_another() {
    let db = testkit::spawn_test_db().await;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(testkit::app_role(&db).await)
        .await
        .unwrap();
    let clients = PgOidcClients::new(pool);
    let mut bi = people("bi-superset");
    assert!(clients.save_module_client(&bi).await.unwrap());
    let found = clients.find(&bi.id).await.unwrap().unwrap();
    assert_eq!(found, bi);

    // Replaced in place: a rotation keeps the previous secret for a while,
    // a removal disables it, an add enables it again.
    let until = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
    let side = bi.settings.module.as_mut().unwrap();
    side.previous_secret = Some(PreviousSecret {
        hash: "hash-1".into(),
        until,
    });
    side.disabled = true;
    bi.auth = ClientAuth::Secret {
        hash: "hash-2".into(),
    };
    assert!(clients.save_module_client(&bi).await.unwrap());
    let found = clients.find(&bi.id).await.unwrap().unwrap();
    assert!(found.disabled());
    assert_eq!(found, bi);
    bi.settings.module.as_mut().unwrap().disabled = false;
    assert!(clients.save_module_client(&bi).await.unwrap());
    assert!(!clients.find(&bi.id).await.unwrap().unwrap().disabled());

    // A client registered by hand is not a module's.
    let hand = OidcClient {
        settings: ClientSettings::default(),
        ..people("grafana")
    };
    clients.upsert(&hand).await.unwrap();
    let refused = clients
        .save_module_client(&people("grafana"))
        .await
        .unwrap();
    assert!(!refused, "a client registered by hand");
    assert_eq!(
        clients
            .find(&hand.id)
            .await
            .unwrap()
            .unwrap()
            .settings
            .module,
        None
    );

    assert_eq!(
        clients.module_clients("bi-superset").await.unwrap(),
        [bi.clone()]
    );
    assert_eq!(
        clients.delete_module_clients("bi-superset").await.unwrap(),
        1
    );
    assert!(clients.find(&bi.id).await.unwrap().is_none());
    assert!(
        clients.find(&hand.id).await.unwrap().is_some(),
        "not a module's"
    );
}
