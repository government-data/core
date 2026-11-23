//! Shared integration-test harness.
//!
//! [`spawn_test_db`] starts a fresh, ephemeral Postgres container via
//! `testcontainers`, applies every one of `infrastructure`'s embedded
//! migrations against it, and hands back a connected pool. Call it once per
//! test: each call gets its own container, so tests run on a schema no other
//! test has ever touched — no manual `docker run`, no shared database, no
//! cross-test bleed. Requires a running Docker daemon.

pub mod lake;
pub mod lake_admin;
pub mod tls;

use sqlx::PgPool;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::postgres::Postgres;

/// The Postgres image the tests run on, as `(name, tag)`.
///
/// It must match the installation's image, `services.postgres.image` in
/// `infra/postgres/docker-compose.yml`, so a migration that is valid on one
/// major version but not the other fails here and not in production.
const POSTGRES_IMAGE: (&str, &str) = ("postgres", "16-alpine");

/// A live ephemeral Postgres instance backing one test.
///
/// Keep this bound for the duration of the test (e.g. `let db =
/// spawn_test_db().await;`) — dropping it stops and removes the container.
/// Binding it to `_` drops it immediately and the pool stops working.
pub struct TestDb {
    _container: ContainerAsync<Postgres>,
    /// A pool connected to the fresh, migrated database.
    pub pool: PgPool,
}

impl std::fmt::Debug for TestDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestDb").finish_non_exhaustive()
    }
}

/// Start a fresh Postgres container, migrate it, and connect a pool to it.
///
/// # Panics
///
/// Panics if Docker is unavailable, the container fails to start, or the
/// connection/migration step fails — any of which means the test environment
/// itself is broken, not the code under test, so failing loudly and early is
/// preferable to a confusing downstream assertion failure.
pub async fn spawn_test_db() -> TestDb {
    let (name, tag) = POSTGRES_IMAGE;
    let container = Postgres::default()
        .with_name(name)
        .with_tag(tag)
        .start()
        .await
        .expect("failed to start ephemeral postgres container — is Docker running?");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("failed to read the mapped postgres port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");

    let pool = PgPool::connect(&url)
        .await
        .expect("failed to connect to the ephemeral postgres container");
    infrastructure::run_migrations(&pool)
        .await
        .expect("failed to run migrations against the ephemeral postgres container");

    TestDb {
        _container: container,
        pool,
    }
}

/// The options to connect as in production: a role without superuser or
/// BYPASSRLS that owns the tenant tables (so RLS only stops it because the
/// tables have FORCE). The container's superuser, the one of
/// [`TestDb::pool`], skips RLS: isolation cannot be tested with it.
///
/// # Panics
///
/// If the role cannot be set up (the test environment is broken).
pub async fn app_role(db: &TestDb) -> sqlx::postgres::PgConnectOptions {
    let admin = &db.pool;
    for sql in [
        "CREATE ROLE app LOGIN PASSWORD 'app' NOSUPERUSER NOBYPASSRLS",
        "GRANT USAGE ON SCHEMA public TO app",
        "GRANT ALL ON ALL TABLES IN SCHEMA public TO app",
        "GRANT ALL ON ALL SEQUENCES IN SCHEMA public TO app",
    ] {
        sqlx::query(sql).execute(admin).await.expect(sql);
    }
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT format('%I.%I', table_schema, table_name) FROM information_schema.columns
          WHERE column_name = 'tenant_id' AND table_schema = 'public'",
    )
    .fetch_all(admin)
    .await
    .expect("list the tenant tables");
    for table in tables {
        sqlx::query(&format!("ALTER TABLE {table} OWNER TO app"))
            .execute(admin)
            .await
            .expect("hand the tables to the app's role");
    }
    (*admin.connect_options())
        .clone()
        .username("app")
        .password("app")
}

/// A new tenant; returns its id.
///
/// # Panics
///
/// If the `slug` is not valid or already exists.
pub async fn new_tenant(db: &TestDb, slug: &str) -> String {
    sqlx::query_scalar("INSERT INTO tenants (slug, name) VALUES ($1, $1) RETURNING id::text")
        .bind(slug)
        .fetch_one(&db.pool)
        .await
        .expect("create the tenant")
}

/// A live ephemeral Dolt server (the catalog, decision 0021), its root
/// reachable from the test (unlike in the stack, where root stays inside
/// the container).
pub struct TestDolt {
    _container: ContainerAsync<testcontainers::GenericImage>,
    /// `mysql://root:root@127.0.0.1:<port>`, without a database.
    pub url: String,
}

impl std::fmt::Debug for TestDolt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestDolt").finish_non_exhaustive()
    }
}

impl TestDolt {
    /// Creates a database, as `infra/dolt/tenant.sh add` does.
    ///
    /// # Panics
    ///
    /// If the server refuses it.
    pub async fn create_database(&self, name: &str) {
        use sqlx::Connection;
        let mut conn = sqlx::MySqlConnection::connect(&self.url)
            .await
            .expect("connect to the ephemeral dolt");
        sqlx::query(&format!("CREATE DATABASE `{name}`"))
            .execute(&mut conn)
            .await
            .expect("create the database");
    }
}

/// Start a fresh Dolt SQL server, the stack's version.
///
/// # Panics
///
/// If Docker is unavailable or the server does not start.
pub async fn spawn_dolt() -> TestDolt {
    use testcontainers::core::{IntoContainerPort, WaitFor};
    use testcontainers::ImageExt;
    let container = testcontainers::GenericImage::new("dolthub/dolt-sql-server", "2.6.1")
        .with_exposed_port(3306.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready for connections"))
        .with_env_var("DOLT_ROOT_PASSWORD", "root")
        .with_env_var("DOLT_ROOT_HOST", "%")
        .start()
        .await
        .expect("failed to start ephemeral dolt container — is Docker running?");
    let port = container
        .get_host_port_ipv4(3306)
        .await
        .expect("failed to read the mapped dolt port");
    TestDolt {
        _container: container,
        url: format!("mysql://root:root@127.0.0.1:{port}"),
    }
}

/// A Dolt server, the stack's version, with `files` copied in before it
/// starts (`(path in the container, contents)`, e.g. a configuration in
/// `/etc/dolt/servercfg.d` and its certificates), its root `root` from any
/// host and the image's init skipped (`.init_completed`), as a server
/// requiring client certificates refuses the image's own `dolt sql`.
/// The container and the port on this machine.
///
/// # Panics
///
/// If Docker is unavailable or the server does not start.
pub async fn spawn_dolt_with(
    files: Vec<(String, Vec<u8>)>,
) -> (ContainerAsync<testcontainers::GenericImage>, u16) {
    use testcontainers::core::{IntoContainerPort, WaitFor};
    use testcontainers::CopyTargetOptions;
    let mut image = testcontainers::GenericImage::new("dolthub/dolt-sql-server", "2.6.1")
        .with_exposed_port(3306.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Server ready"))
        .with_env_var("DOLT_ROOT_PASSWORD", "root")
        .with_env_var("DOLT_ROOT_HOST", "%")
        .with_copy_to("/var/lib/dolt/.init_completed", Vec::new());
    for (path, contents) in files {
        image = image.with_copy_to(CopyTargetOptions::new(path).with_mode(0o644), contents);
    }
    let container = image
        .start()
        .await
        .expect("failed to start ephemeral dolt container — is Docker running?");
    let port = container
        .get_host_port_ipv4(3306)
        .await
        .expect("failed to read the mapped dolt port");
    (container, port)
}
