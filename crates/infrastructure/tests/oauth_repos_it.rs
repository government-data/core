//! Integration tests for the OAuth Postgres adapters: the one-shot pending
//! auth store and the identity-linking repository, against a real, ephemeral,
//! freshly migrated Postgres via `testkit`.

use std::time::{Duration, SystemTime};

use auth_gateway::{
    AdminRepository, NewAdmin, OAuthIdentityRepository, PasswordHash, PendingAuthStore,
    PendingAuthorization, ProviderId,
};
use domain::{Email, Role};
use infrastructure::{PgAdminRepository, PgOAuthIdentityRepository, PgPendingAuthStore};

fn pending(state: &str) -> PendingAuthorization {
    PendingAuthorization {
        state: state.to_string(),
        provider: ProviderId::parse("google").unwrap(),
        nonce: "nonce-1".to_string(),
        code_verifier: "verifier-1".to_string(),
        redirect_uri: "https://app.example.com/cb".to_string(),
        created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        link_admin: None,
    }
}

#[tokio::test]
async fn pending_insert_then_consume_round_trips() {
    let db = testkit::spawn_test_db().await;
    let store = PgPendingAuthStore::new(db.pool.clone());

    store.insert(&pending("state-1")).await.unwrap();
    let consumed = store.consume("state-1").await.unwrap().unwrap();
    assert_eq!(consumed.provider, ProviderId::parse("google").unwrap());
    assert_eq!(consumed.nonce, "nonce-1");
    assert_eq!(consumed.code_verifier, "verifier-1");
    assert_eq!(consumed.redirect_uri, "https://app.example.com/cb");
}

#[tokio::test]
async fn consuming_a_state_is_one_shot() {
    let db = testkit::spawn_test_db().await;
    let store = PgPendingAuthStore::new(db.pool.clone());

    store.insert(&pending("state-1")).await.unwrap();
    assert!(store.consume("state-1").await.unwrap().is_some());
    // A replay finds nothing — the row was deleted by the first consume.
    assert!(store.consume("state-1").await.unwrap().is_none());
}

#[tokio::test]
async fn consuming_an_unknown_state_is_none() {
    let db = testkit::spawn_test_db().await;
    let store = PgPendingAuthStore::new(db.pool.clone());
    assert!(store.consume("never-inserted").await.unwrap().is_none());
}

async fn seed_admin(pool: &sqlx::PgPool, email: &str) -> domain::AdminId {
    PgAdminRepository::new(pool.clone())
        .insert(&NewAdmin {
            email: Email::parse(email).unwrap(),
            password_hash: PasswordHash::from_encoded("hash"),
            role: Role::admin(),
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn identity_link_then_find_round_trips() {
    let db = testkit::spawn_test_db().await;
    let admin_id = seed_admin(&db.pool, "admin@example.com").await;
    let repo = PgOAuthIdentityRepository::new(db.pool.clone());
    let google = ProviderId::parse("google").unwrap();

    assert!(repo.find_admin(&google, "sub-1").await.unwrap().is_none());

    repo.link(&google, "sub-1", Some("admin@example.com"), &admin_id)
        .await
        .unwrap();
    assert_eq!(
        repo.find_admin(&google, "sub-1").await.unwrap(),
        Some(admin_id)
    );
}

#[tokio::test]
async fn linking_is_idempotent() {
    let db = testkit::spawn_test_db().await;
    let admin_id = seed_admin(&db.pool, "admin@example.com").await;
    let repo = PgOAuthIdentityRepository::new(db.pool.clone());
    let google = ProviderId::parse("google").unwrap();

    repo.link(&google, "sub-1", Some("admin@example.com"), &admin_id)
        .await
        .unwrap();
    // Re-linking the same identity refreshes the email rather than erroring.
    repo.link(&google, "sub-1", Some("new@example.com"), &admin_id)
        .await
        .unwrap();
    assert_eq!(
        repo.find_admin(&google, "sub-1").await.unwrap(),
        Some(admin_id)
    );
}

#[tokio::test]
async fn deleting_the_admin_cascades_to_their_identities() {
    let db = testkit::spawn_test_db().await;
    let admin_id = seed_admin(&db.pool, "admin@example.com").await;
    let repo = PgOAuthIdentityRepository::new(db.pool.clone());
    let google = ProviderId::parse("google").unwrap();

    repo.link(&google, "sub-1", Some("admin@example.com"), &admin_id)
        .await
        .unwrap();

    sqlx::query("DELETE FROM admin_users WHERE id = $1::uuid")
        .bind(admin_id.as_str())
        .execute(&db.pool)
        .await
        .unwrap();

    assert!(repo.find_admin(&google, "sub-1").await.unwrap().is_none());
}

#[tokio::test]
async fn a_link_flow_keeps_its_admin_through_the_pending_store() {
    let db = testkit::spawn_test_db().await;
    let admin_id = seed_admin(&db.pool, "admin@example.com").await;
    let store = PgPendingAuthStore::new(db.pool.clone());

    store
        .insert(&PendingAuthorization {
            link_admin: Some(admin_id.clone()),
            ..pending("state-1")
        })
        .await
        .unwrap();
    let consumed = store.consume("state-1").await.unwrap().unwrap();
    assert_eq!(consumed.link_admin, Some(admin_id));
}

#[tokio::test]
async fn linking_never_moves_an_identity_to_another_admin() {
    let db = testkit::spawn_test_db().await;
    let ana = seed_admin(&db.pool, "ana@example.com").await;
    let luis = seed_admin(&db.pool, "luis@example.com").await;
    let repo = PgOAuthIdentityRepository::new(db.pool.clone());
    let google = ProviderId::parse("google").unwrap();

    repo.link(&google, "sub-1", None, &ana).await.unwrap();
    repo.link(&google, "sub-1", Some("luis@example.com"), &luis)
        .await
        .unwrap();
    assert_eq!(repo.find_admin(&google, "sub-1").await.unwrap(), Some(ana));
}

#[tokio::test]
async fn list_and_unlink_are_per_admin() {
    let db = testkit::spawn_test_db().await;
    let ana = seed_admin(&db.pool, "ana@example.com").await;
    let luis = seed_admin(&db.pool, "luis@example.com").await;
    let repo = PgOAuthIdentityRepository::new(db.pool.clone());
    let google = ProviderId::parse("google").unwrap();
    let entra = ProviderId::parse("microsoft").unwrap();

    repo.link(&google, "g-1", Some("ana@gmail.com"), &ana)
        .await
        .unwrap();
    repo.link(&entra, "m-1", None, &ana).await.unwrap();
    repo.link(&google, "g-2", None, &luis).await.unwrap();

    let mine = repo.list_for_admin(&ana).await.unwrap();
    assert_eq!(mine.len(), 2);
    assert!(mine
        .iter()
        .any(|i| i.provider == google && i.email.as_deref() == Some("ana@gmail.com")));
    assert!(mine
        .iter()
        .any(|i| i.provider == entra && i.email.is_none()));

    // Luis cannot remove Ana's link; Ana can.
    assert!(!repo.unlink(&luis, &google, "g-1").await.unwrap());
    assert!(repo.unlink(&ana, &google, "g-1").await.unwrap());
    assert_eq!(repo.list_for_admin(&ana).await.unwrap().len(), 1);
    assert_eq!(repo.list_for_admin(&luis).await.unwrap().len(), 1);
}
