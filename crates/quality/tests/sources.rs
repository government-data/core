//! The sources' store on an ephemeral PostgreSQL, as the application's role
//! with row security per tenant (decision 0036): secrets are stored only
//! sealed and never read back for people; an edit that leaves a secret out
//! keeps it, one that writes a secret keeps the previous version; another
//! tenant sees nothing. Synthetic values only.

use std::collections::BTreeMap;

use contracts::ModuleRegistry;
use infrastructure::module_migrations::PgModuleMigrator;
use quality::module::QualityModule;
use quality::source::{Source, SourceError};
use quality::source_store::{self, Put, Sealer, StoreError};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

const A: &str = "00000000-0000-0000-0000-00000000000a";
const B: &str = "00000000-0000-0000-0000-00000000000b";

/// Reverses the text behind a prefix: never the plain text in a column.
struct Mirror;
impl Sealer for Mirror {
    fn seal(&self, plain: &str) -> Result<String, String> {
        Ok(format!(
            "sealed:{}",
            plain.chars().rev().collect::<String>()
        ))
    }
    fn open(&self, sealed: &str) -> Result<String, String> {
        let body = sealed.strip_prefix("sealed:").ok_or("not sealed")?;
        Ok(body.chars().rev().collect())
    }
}

struct Fixture {
    db: testkit::TestDb,
    app: PgPool,
}

async fn fixture() -> Fixture {
    let db = testkit::spawn_test_db().await;
    sqlx::query("CREATE ROLE qd_queue LOGIN PASSWORD 'queue'")
        .execute(&db.pool)
        .await
        .unwrap();
    let mut migrator = PgModuleMigrator::load(&db.pool).await.unwrap();
    ModuleRegistry::new()
        .register(QualityModule::default())
        .run_migrations(&mut migrator)
        .unwrap();
    migrator.run(&db.pool).await.unwrap();
    let options = testkit::app_role(&db).await;
    for sql in [
        "GRANT USAGE ON SCHEMA quality TO app",
        "ALTER TABLE quality.sources OWNER TO app",
    ] {
        sqlx::query(sql).execute(&db.pool).await.unwrap();
    }
    let app = infrastructure::db::with_tenancy(PgPoolOptions::new(), tenancy::Mode::Multiple)
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    Fixture { db, app }
}

fn tenant(id: &str) -> tenancy::Tenant {
    tenancy::Tenant {
        id: id.into(),
        slug: String::new(),
    }
}

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn core(secrets: &[(&str, &str)]) -> Source {
    Source {
        name: "core".into(),
        driver: "postgres".into(),
        fields: map(&[("host", "db.example"), ("database", "core")]),
        secrets: map(secrets),
    }
}

#[tokio::test]
async fn secrets_are_sealed_kept_on_edit_and_rotated_in_two_versions() {
    let f = fixture().await;
    tenancy::within(tenant(A), async {
        let first = core(&[("user", "reader"), ("password", "first-synthetic")]);
        assert_eq!(
            source_store::put(&f.app, &Mirror, &first).await.unwrap(),
            Put::Created
        );

        // Never in the clear in the table, never in what people read.
        let stored: String = sqlx::query_scalar("SELECT secrets FROM quality.sources")
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
        assert!(!stored.contains("first-synthetic"));
        let views = source_store::list(&f.app, &Mirror).await.unwrap();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].secrets, ["password", "user"]);
        assert!(!views[0].previous);
        assert!(!serde_json::to_string(&views)
            .unwrap()
            .contains("first-synthetic"));

        // An edit of a field, its secrets left out: they stay, no rotation.
        let mut edit = core(&[]);
        edit.fields.insert("port".into(), "6432".into());
        assert_eq!(
            source_store::put(&f.app, &Mirror, &edit).await.unwrap(),
            Put::Changed
        );
        let c = source_store::credentials(&f.app, &Mirror, "core")
            .await
            .unwrap();
        assert_eq!(c.source.fields["port"], "6432");
        assert_eq!(c.source.secrets["password"], "first-synthetic");
        assert!(c.previous.is_none());

        // A new password: the current one becomes the previous.
        assert_eq!(
            source_store::put(&f.app, &Mirror, &core(&[("password", "second-synthetic")]))
                .await
                .unwrap(),
            Put::SecretsChanged
        );
        let c = source_store::credentials(&f.app, &Mirror, "core")
            .await
            .unwrap();
        assert_eq!(c.source.secrets["password"], "second-synthetic");
        assert_eq!(c.source.secrets["user"], "reader");
        assert_eq!(c.previous.unwrap()["password"], "first-synthetic");
        assert!(source_store::list(&f.app, &Mirror).await.unwrap()[0].previous);
    })
    .await;
}

#[tokio::test]
async fn a_source_that_does_not_hold_together_is_not_stored() {
    let f = fixture().await;
    tenancy::within(tenant(A), async {
        let missing = core(&[("user", "reader")]);
        assert!(matches!(
            source_store::put(&f.app, &Mirror, &missing).await,
            Err(StoreError::Invalid(e)) if e == [SourceError::Missing("password".into())]
        ));
        assert!(source_store::list(&f.app, &Mirror)
            .await
            .unwrap()
            .is_empty());
        assert!(matches!(
            source_store::credentials(&f.app, &Mirror, "core").await,
            Err(StoreError::NotFound)
        ));
    })
    .await;
}

#[tokio::test]
async fn another_tenant_sees_none_of_them_and_may_use_the_same_name() {
    let f = fixture().await;
    let secrets = [("user", "reader"), ("password", "synthetic")];
    tenancy::within(tenant(A), async {
        source_store::put(&f.app, &Mirror, &core(&secrets))
            .await
            .unwrap();
    })
    .await;
    tenancy::within(tenant(B), async {
        assert!(source_store::list(&f.app, &Mirror)
            .await
            .unwrap()
            .is_empty());
        assert!(matches!(
            source_store::credentials(&f.app, &Mirror, "core").await,
            Err(StoreError::NotFound)
        ));
        source_store::put(&f.app, &Mirror, &core(&secrets))
            .await
            .unwrap();
        assert_eq!(source_store::list(&f.app, &Mirror).await.unwrap().len(), 1);
    })
    .await;
}

#[tokio::test]
async fn a_deletion_and_an_approval_of_the_same_source_go_one_after_the_other() {
    let f = fixture().await;
    let secrets = [("user", "reader"), ("password", "synthetic")];
    tenancy::within(tenant(A), async {
        source_store::put(&f.app, &Mirror, &core(&secrets))
            .await
            .unwrap();
        // A deletion holds it: an approval that reads it waits.
        let deleting = source_store::hold(&f.app, "core", true)
            .await
            .unwrap()
            .unwrap();
        let waiting = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            source_store::hold(&f.app, "core", false),
        )
        .await;
        assert!(waiting.is_err(), "the approval waits for the deletion");
        // Deleted: the approval then finds no source, and is refused.
        assert!(source_store::delete_held(deleting, "core").await.unwrap());
        assert!(source_store::hold(&f.app, "core", false)
            .await
            .unwrap()
            .is_none());
    })
    .await;
}
