//! The database connections among the global secrets, with the real service and an
//! ephemeral Postgres: they are stored encrypted whole and in plain text only
//! what is not secret, editing them without typing the password keeps the
//! stored one, and a single-value secret stays the same. Needs Docker.

use std::sync::Arc;

use application::{PlatformService, SecretCipher};
use iam::Actor;
use infrastructure::{PgPlatformRepository, SecretBox};
use platform::PlatformRepository;
use serde_json::{json, Value};

struct Cipher(SecretBox);

impl SecretCipher for Cipher {
    fn encrypt(&self, plain: &str) -> Result<String, String> {
        self.0.encrypt(plain)
    }
    fn decrypt(&self, sealed: &str) -> Result<String, String> {
        self.0.decrypt(sealed)
    }
}

#[tokio::test]
async fn a_connection_keeps_its_secrets_out_of_sight_and_across_edits() {
    let db = testkit::spawn_test_db().await;
    let repo = PgPlatformRepository::new(db.pool.clone());
    let admin_id: String = sqlx::query_scalar(
        "INSERT INTO admin_users (email, password_hash, role) VALUES ('ana@company.com', 'x', 'admin')
         RETURNING id::text",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let admin = Actor::account(admin_id, true, Default::default());
    let platform = PlatformService::new(Arc::new(repo.clone()))
        .with_cipher(Arc::new(Cipher(SecretBox::from_key(&[5; 32]).unwrap())));
    let opened = |id: &platform::SecretId| {
        let repo = repo.clone();
        let id = id.clone();
        async move {
            let sealed = repo.secret_values(std::slice::from_ref(&id)).await.unwrap()[&id].clone();
            let plain = SecretBox::from_key(&[5; 32])
                .unwrap()
                .decrypt(&sealed)
                .unwrap();
            serde_json::from_str::<Value>(&plain).unwrap()
        }
    };

    let conn = json!({"kind": "database", "engine": "sqlserver", "fields": {
        "host": "Hosthidra", "port": "1433", "database": "DMInformes", "user": "diego", "password": "clave-1"}});
    let saved = platform
        .create_secret(&admin, "BD_INFORMES", "SQL Server de informes", &conn)
        .await
        .unwrap();
    assert_eq!(saved.kind, "database");
    assert_eq!(saved.meta["engine"], "sqlserver");
    assert_eq!(saved.meta["fields"]["host"], "Hosthidra");
    assert_eq!(saved.meta["secrets"], json!(["password"]));
    // The password is not left in plain text.
    assert!(!saved.meta.to_string().contains("clave-1"));
    assert_eq!(opened(&saved.id).await["fields"]["password"], "clave-1");

    // Editing without typing the password keeps it; the rest changes.
    let edit = json!({"kind": "database", "engine": "sqlserver", "fields": {
        "host": "HOSTURANO", "port": "1433", "database": "DMBdAgencias", "user": "tarea"}});
    let updated = platform
        .update_secret(&admin, &saved.id, None, Some(&edit))
        .await
        .unwrap();
    assert_eq!(updated.meta["fields"]["host"], "HOSTURANO");
    assert_eq!(updated.meta["secrets"], json!(["password"]));
    let value = opened(&saved.id).await;
    assert_eq!(value["fields"]["password"], "clave-1");
    assert_eq!(value["fields"]["user"], "tarea");

    // A port that is not a number is not stored.
    let bad =
        json!({"kind": "database", "engine": "sqlserver", "fields": {"host": "h", "port": "uno"}});
    assert!(platform
        .create_secret(&admin, "BD_MALA", "", &bad)
        .await
        .is_err());

    // A single-value secret stays the same.
    let plain = platform
        .create_secret(&admin, "API_KEY", "", &json!("abc"))
        .await
        .unwrap();
    assert_eq!(plain.kind, "value");
    assert_eq!(plain.meta, json!({}));
    assert_eq!(opened(&plain.id).await, json!("abc"));
}
