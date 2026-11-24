//! A module's actions and managed policies in IAM (quality-sdk decision
//! 0009) end to end, with the real services on PostgreSQL: as the update
//! agent tells them, they are in the catalog, a managed policy of the module
//! reaches an account and shows its menu entry, a custom policy may name its
//! actions, and a removed module's policies stay, assigned, granting
//! nothing. Needs Docker.

use std::sync::Arc;

use application::{InstalledModule, PlatformService, UpdatesService};
use domain::Role;
use infrastructure::{PgMaintenanceStore, PgPlatformRepository};
use serde_json::json;
use sqlx::PgPool;

#[path = "../../server/src/updates_wiring.rs"]
mod updates_wiring;
use updates_wiring::Store;

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

/// `bi-superset` as the agent tells it.
fn analytics() -> InstalledModule {
    serde_json::from_value(json!({
        "name": "bi-superset", "version": "1.0.0", "route_path": "/bi",
        "menu": { "label": { "en": "Analytics", "es": "Análisis" },
                  "path": { "en": "/bi/", "es": "/bi/" }, "requires": "bi-superset:Author" },
        "permissions": {
            "actions": [
                { "name": "bi-superset:Author",
                  "description": { "en": "Create charts", "es": "Crear gráficos" } },
                { "name": "bi-superset:Administer",
                  "description": { "en": "Administer BI", "es": "Administrar BI" } }],
            "policies": [
                { "name": "bi-superset:Authors",
                  "description": { "en": "BI authors", "es": "Autores de BI" },
                  "actions": ["bi-superset:Author"] },
                { "name": "bi-superset:Administrators",
                  "description": { "en": "BI administrators", "es": "Administradores de BI" },
                  "actions": ["bi-superset:Author", "bi-superset:Administer"] }] }
    }))
    .unwrap()
}

#[tokio::test]
async fn a_modules_policies_are_assigned_as_the_cores_own() {
    let db = testkit::spawn_test_db().await;
    let pool = db.pool.clone();
    let updates = UpdatesService::new(Arc::new(Store(PgMaintenanceStore::new(pool.clone()))));
    let platform = PlatformService::new(Arc::new(PgPlatformRepository::new(pool.clone())))
        .with_updates(updates.clone());
    let root_id = account(&pool, "root@example.com", "admin").await;
    let ana_id = account(&pool, "ana@example.com", "user").await;
    let root = platform
        .actor(&root_id, &Role::admin(), None)
        .await
        .unwrap();
    let user = Role::parse("user").unwrap();

    updates.set_modules(vec![analytics()]).await.unwrap();
    let actions = platform.module_actions().await.unwrap();
    assert_eq!(actions.len(), 2);
    assert!(actions.iter().all(|a| a.module.installed));
    let listed = platform.list_policies(&root).await.unwrap();
    let authors = listed
        .iter()
        .find(|p| p.policy.name == "bi-superset:Authors")
        .unwrap();
    assert!(authors.policy.managed);
    assert_eq!(authors.module.as_ref().unwrap().name, "bi-superset");

    // Assigned to an account, the managed policy gives the module's action.
    let names = ["bi-superset:Authors".to_string()];
    platform
        .set_user_policies(&root, &ana_id, &names)
        .await
        .unwrap();
    let ana = platform.actor(&ana_id, &user, None).await.unwrap();
    assert!(ana.can_some("bi-superset:Author"));
    assert!(!ana.can_some("bi-superset:Administer"));

    // A custom policy may name a module's actions, on `*` only; a managed
    // one of a module is neither changed nor removed.
    let author = iam::Statement::allow(&["bi-superset:Author"], &["*"]);
    platform
        .put_policy(&root, "bi-writers", "", vec![author])
        .await
        .unwrap();
    let on_pipeline = iam::Statement::allow(&["bi-superset:Author"], &["pipeline/p"]);
    let refused = platform
        .put_policy(&root, "bad", "", vec![on_pipeline])
        .await;
    assert!(refused.is_err());
    let delete = platform.delete_policy(&root, "bi-superset:Authors").await;
    assert!(delete.is_err());

    // Removed: its policies stay and stay assigned, granting nothing.
    updates.set_modules(vec![]).await.unwrap();
    platform.invalidate_permissions();
    let ana = platform.actor(&ana_id, &user, None).await.unwrap();
    assert!(!ana.can_some("bi-superset:Administer"));
    assert!(
        !ana.permissions.can("bi-superset:Author", "*"),
        "the removed module's policy grants nothing"
    );
    let listed = platform.list_policies(&root).await.unwrap();
    let authors = listed
        .iter()
        .find(|p| p.policy.name == "bi-superset:Authors")
        .unwrap();
    assert!(!authors.module.as_ref().unwrap().installed);
    assert_eq!(authors.attached, 1, "its assignment stays");
    platform
        .set_user_policies(&root, &ana_id, &names)
        .await
        .unwrap();
}
