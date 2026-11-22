//! Recovering a restored company: the new administrator signs in, the others
//! choose their password again, and no session or external link is left.

use auth_gateway::{AdminRepository, NewAdmin, PasswordHash};
use domain::{Email, Role};
use infrastructure::PgAdminRepository;

#[tokio::test]
async fn a_restored_company_is_handed_to_the_new_admin_only() {
    let db = testkit::spawn_test_db().await;
    let repo = PgAdminRepository::new(db.pool.clone());
    let alta = |email: &str, hash: PasswordHash| NewAdmin {
        email: Email::parse(email).unwrap(),
        password_hash: hash,
        role: Role::admin(),
    };
    let empleado = repo
        .insert(&alta(
            "empleado@coop.co",
            PasswordHash::from_encoded("$argon2id$viejo"),
        ))
        .await
        .unwrap();
    repo.insert(&alta("ldap@coop.co", PasswordHash::directory_managed()))
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO admin_sessions (token, csrf_token, admin_id, created_at, last_seen_at, absolute_expires_at)
         VALUES ('t', 'c', $1::uuid, now(), now(), now() + interval '1 day')",
    )
    .bind(empleado.as_str())
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO admin_oauth_identities (provider, subject, admin_id, email)
         VALUES ('google', 's', $1::uuid, 'empleado@example.org')",
    )
    .bind(empleado.as_str())
    .execute(&db.pool)
    .await
    .unwrap();

    let created = Email::parse("owner@coop.co").unwrap();
    let hecho = infrastructure::admin_recovery::recover_admin(
        &db.pool,
        &created,
        &PasswordHash::from_encoded("$argon2id$nueva"),
    )
    .await
    .unwrap();
    assert!(hecho.created);
    assert_eq!(hecho.accounts_to_reset, 1);
    assert_eq!(hecho.sessions_closed, 1);
    assert_eq!(hecho.links_removed, 1);

    let owner = repo.find_by_email(&created).await.unwrap().unwrap();
    assert_eq!(owner.password_hash.as_str(), "$argon2id$nueva");
    assert_eq!(owner.role, Role::admin());
    let emp = repo
        .find_by_email(&Email::parse("empleado@coop.co").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(emp.password_hash.is_pending_activation());
    let ldap = repo
        .find_by_email(&Email::parse("ldap@coop.co").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(ldap.password_hash.is_directory_managed());

    // Again with the same email: it updates it, it does not duplicate it.
    let again = infrastructure::admin_recovery::recover_admin(
        &db.pool,
        &created,
        &PasswordHash::from_encoded("$argon2id$again"),
    )
    .await
    .unwrap();
    assert!(!again.created);
}
