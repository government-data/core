//! [`PgPasswordResetStore`] against a real, freshly migrated Postgres via
//! `testkit`. Requires a running Docker daemon.

use std::time::{Duration, SystemTime};

use auth_gateway::{
    AdminRepository, CsrfToken, LockoutState, NewAdmin, PasswordHash, PasswordResetStore, Session,
    SessionRepository, SessionToken,
};
use domain::{Email, Role};
use infrastructure::{PgAdminRepository, PgPasswordResetStore, PgSessionRepository};

async fn seed(pool: &sqlx::PgPool, email: &str, hash: PasswordHash) -> domain::AdminId {
    PgAdminRepository::new(pool.clone())
        .insert(&NewAdmin {
            email: Email::parse(email).unwrap(),
            password_hash: hash,
            role: Role::admin(),
        })
        .await
        .unwrap()
}

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_900_000_000)
}

#[tokio::test]
async fn redeeming_sets_the_password_clears_the_lock_and_ends_the_sessions_once() {
    let db = testkit::spawn_test_db().await;
    let admins = PgAdminRepository::new(db.pool.clone());
    let sessions = PgSessionRepository::new(db.pool.clone());
    let store = PgPasswordResetStore::new(db.pool.clone());
    let id = seed(
        &db.pool,
        "ana@example.com",
        PasswordHash::from_encoded("old"),
    )
    .await;
    admins
        .update_lockout(
            &id,
            &LockoutState {
                failed_attempts: 7,
                locked_until: Some(now() + Duration::from_secs(600)),
            },
        )
        .await
        .unwrap();
    sessions
        .insert(&Session {
            token: SessionToken::from_raw("s1"),
            admin_id: id.clone(),
            role: Role::admin(),
            csrf_token: CsrfToken::from_raw("c1"),
            created_at: now(),
            last_seen_at: now(),
            absolute_expires_at: now() + Duration::from_secs(3600),
        })
        .await
        .unwrap();

    store
        .issue(&id, "hash-1", now() + Duration::from_secs(1800))
        .await
        .unwrap();
    assert_eq!(store.find("hash-1", now()).await.unwrap(), Some(id.clone()));

    let new = PasswordHash::from_encoded("new");
    assert_eq!(
        store.redeem("hash-1", &new, now()).await.unwrap(),
        Some(id.clone())
    );
    let account = admins.find_by_id(&id).await.unwrap().unwrap();
    assert_eq!(account.password_hash.as_str(), "new");
    assert_eq!(account.lockout, LockoutState::clear());
    assert!(sessions
        .find(&SessionToken::from_raw("s1"))
        .await
        .unwrap()
        .is_none());

    assert_eq!(store.redeem("hash-1", &new, now()).await.unwrap(), None);
}

#[tokio::test]
async fn only_the_newest_unexpired_link_works() {
    let db = testkit::spawn_test_db().await;
    let store = PgPasswordResetStore::new(db.pool.clone());
    let id = seed(
        &db.pool,
        "ana@example.com",
        PasswordHash::from_encoded("old"),
    )
    .await;
    let expires = now() + Duration::from_secs(1800);

    store.issue(&id, "first", expires).await.unwrap();
    store.issue(&id, "second", expires).await.unwrap();
    assert_eq!(store.find("first", now()).await.unwrap(), None);
    assert_eq!(store.find("second", now()).await.unwrap(), Some(id));

    let late = expires + Duration::from_secs(1);
    assert_eq!(store.find("second", late).await.unwrap(), None);
    assert_eq!(
        store
            .redeem("second", &PasswordHash::from_encoded("new"), late)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn a_directory_account_never_gets_a_local_password() {
    let db = testkit::spawn_test_db().await;
    let store = PgPasswordResetStore::new(db.pool.clone());
    let id = seed(
        &db.pool,
        "luis@corp.example",
        PasswordHash::directory_managed(),
    )
    .await;
    store
        .issue(&id, "h", now() + Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(
        store
            .redeem("h", &PasswordHash::from_encoded("new"), now())
            .await
            .unwrap(),
        None
    );
    let account = PgAdminRepository::new(db.pool.clone())
        .find_by_id(&id)
        .await
        .unwrap()
        .unwrap();
    assert!(account.password_hash.is_directory_managed());
}
