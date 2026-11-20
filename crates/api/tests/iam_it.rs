//! IAM end to end (ADR 0016), with the real service and an ephemeral
//! Postgres: a custom policy reaches an account through a group, a directory
//! group feeds a group at sign-in, secrets are listed by the policy's scope,
//! and a service account's token acts for its owner, narrowed by its own
//! policies. Needs Docker.

use std::sync::Arc;

use application::{GroupChange, PlatformService};
use auth_gateway::{DirectoryAccounts, ServiceTokens};
use domain::{AdminId, Role};
use iam::{Effect, Statement};
use infrastructure::{
    PgDirectoryAccounts, PgPlatformRepository, PgServiceTokens, ServiceTokenMint,
};
use serde_json::json;
use sqlx::PgPool;

async fn account(pool: &PgPool, email: &str, role: &str) -> String {
    sqlx::query_scalar(
        "INSERT INTO admin_users (email, password_hash, role) VALUES ($1, 'x', $2)
         RETURNING id::text",
    )
    .bind(email)
    .bind(role)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn user() -> Role {
    Role::parse("user").unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // TODO(0034): split
async fn policies_reach_accounts_through_groups_directories_and_tokens() {
    let db = testkit::spawn_test_db().await;
    let pool = db.pool.clone();
    let platform = PlatformService::new(Arc::new(PgPlatformRepository::new(pool.clone())))
        .with_tokens(Arc::new(ServiceTokenMint));
    let root_id = account(&pool, "root@company.com", "admin").await;
    let ana_id = account(&pool, "ana@company.com", "user").await;
    let luis_id = account(&pool, "luis@company.com", "user").await;
    let root = platform
        .actor(&root_id, &Role::admin(), None)
        .await
        .unwrap();

    // Without policies, a user may do nothing.
    let ana = platform.actor(&ana_id, &user(), None).await.unwrap();
    assert!(ana.actions().is_empty());
    assert!(platform.list_secrets(&ana).await.is_err());

    // A custom policy: the test secrets, never the production one.
    platform
        .put_policy(
            &root,
            "test-secrets",
            "The test databases",
            vec![
                Statement::allow(&["admin:ManageSecrets"], &["secret/BD_*"]),
                Statement {
                    effect: Effect::Deny,
                    ..Statement::allow(&["admin:ManageSecrets"], &["secret/BD_PROD"])
                },
            ],
        )
        .await
        .unwrap();
    assert!(platform
        .put_policy(
            &root,
            "quality-viewer",
            "",
            vec![Statement::allow(&["*"], &["*"])]
        )
        .await
        .is_err());

    // Through a group, with a managed policy too.
    let group = platform
        .create_group(
            &root,
            &GroupChange {
                name: "data-team".into(),
                description: "".into(),
                directory_groups: vec!["CN=Quality,OU=Groups,DC=company,DC=com".into()],
                members: vec![ana_id.clone()],
                policies: vec!["test-secrets".into(), "auditor".into()],
            },
        )
        .await
        .unwrap();
    let ana = platform.actor(&ana_id, &user(), None).await.unwrap();
    assert!(ana.can("admin:ManageSecrets", "secret/BD_TEST"));
    assert!(!ana.can("admin:ManageSecrets", "secret/BD_PROD"));
    assert!(ana.may("admin:ViewAudit"));
    assert!(!ana.may("admin:Configure"));

    // Secrets are listed by the scope, in SQL.
    for name in ["BD_TEST", "BD_PROD", "API_KEY"] {
        platform
            .create_secret(&root, name, "", &json!("x"))
            .await
            .unwrap();
    }
    let names: Vec<String> = platform
        .list_secrets(&ana)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, ["BD_TEST"]);
    assert!(platform
        .create_secret(&ana, "API_OTHER", "", &json!("x"))
        .await
        .is_err());

    // The directory feeds the group at sign-in.
    let directory = PgDirectoryAccounts::new(pool.clone());
    let groups = vec!["cn=quality,ou=groups,dc=company,dc=com".to_string()];
    assert!(directory.grants_access(&groups).await.unwrap());
    assert!(!directory
        .grants_access(&["cn=other,dc=company,dc=com".to_string()])
        .await
        .unwrap());
    directory
        .sync_groups(&AdminId::new(&luis_id), &groups)
        .await
        .unwrap();
    platform.invalidate_permissions(None);
    let luis = platform.actor(&luis_id, &user(), None).await.unwrap();
    assert!(luis.may("admin:ViewAudit"));
    let listed = platform.list_groups(&root).await.unwrap();
    let members: Vec<(String, String)> = listed[0]
        .members
        .iter()
        .map(|m| (m.email.clone(), m.source.clone()))
        .collect();
    assert_eq!(
        members,
        [
            ("ana@company.com".to_string(), "manual".to_string()),
            ("luis@company.com".to_string(), "directory".to_string())
        ]
    );

    // A service account of Ana's, narrowed to the audit log.
    let ana = platform.actor(&ana_id, &user(), None).await.unwrap();
    let created = platform
        .create_service_account(&ana, "audit-export", &["auditor".to_string()], None)
        .await
        .unwrap();
    let tokens = PgServiceTokens::new(pool.clone());
    let principal = tokens.verify(&created.token).await.unwrap().unwrap();
    assert_eq!(principal.owner_id.as_str(), ana_id);
    assert!(tokens
        .verify(&format!("{}.wrong", created.access_key))
        .await
        .unwrap()
        .is_none());
    let sa = platform
        .actor(&ana_id, &user(), Some(&principal.service_account_id))
        .await
        .unwrap();
    assert!(sa.may("admin:ViewAudit"));
    assert!(!sa.can("admin:ManageSecrets", "secret/BD_TEST"));
    // A service account cannot make others.
    assert!(platform
        .create_service_account(&sa, "again", &[], None)
        .await
        .is_err());
    // Luis does not see Ana's; the administrator does.
    let luis = platform.actor(&luis_id, &user(), None).await.unwrap();
    assert!(platform
        .list_service_accounts(&luis)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        platform.list_service_accounts(&root).await.unwrap().len(),
        1
    );

    // Removing the group takes its policies away.
    platform.delete_group(&root, &group).await.unwrap();
    let ana = platform.actor(&ana_id, &user(), None).await.unwrap();
    assert!(ana.actions().is_empty());
    platform
        .delete_service_account(&ana, &created.id)
        .await
        .unwrap();
    assert!(tokens.verify(&created.token).await.unwrap().is_none());
}
