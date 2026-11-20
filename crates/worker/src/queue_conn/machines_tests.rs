//! The queue asks which machine a session is (decision 0066, points 4 and
//! 6; `crates/quality/migrations/0024_queue_machines.sql`), on the
//! PostgreSQL of `test_pg`, migrated as the application's role over
//! `sslmode=verify-full` as the API connects (`PgConfig`): an approved
//! machine takes jobs; a pending one connects and takes none, told why; a
//! draining one renews and finishes its job and takes no new one through
//! the functions; a retired
//! or unknown one is refused, and retiring ends its open sessions. Synthetic
//! jobs only. A session without a certificate (development's password) is
//! `quality`'s `tests/queue.rs`.

use std::time::Duration;

use auth_gateway::MachineSessions;
use contracts::ModuleRegistry;
use infrastructure::db::PgConfig;
use infrastructure::module_migrations::PgModuleMigrator;
use infrastructure::PgMachineSessions;
use quality::module::QualityModule;
use quality::queue::{Queue, Timing};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgConnection, PgPool};

use super::options;
use super::test_pg::{tls_postgres, APP_PASSWORD};
use crate::machine::MachineFiles;
use crate::testing::{write_machine, TempDir, TestCa};

const MAIN: &str = "00000000-0000-0000-0000-000000000001";

/// The database, the application's pool on it and the test's CA.
struct World {
    _pg: testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>,
    ca: TestCa,
    ca_dir: TempDir,
    port: u16,
    app: PgPool,
}

/// The application's URL: its password, over TLS verifying the server.
fn app_url(port: u16, ca: &std::path::Path, ssl: &str) -> String {
    let ca = ca.display();
    format!(
        "postgres://app:{APP_PASSWORD}@localhost:{port}/postgres?sslmode={ssl}&sslrootcert={ca}"
    )
}

async fn world() -> World {
    let (ca, ca_dir) = (TestCa::new(), TempDir::new());
    std::fs::write(ca_dir.path().join("ca.crt"), &ca.pem).unwrap();
    let (pg, port) = tls_postgres(&ca).await;
    let config = PgConfig {
        url: app_url(port, &ca_dir.path().join("ca.crt"), "verify-full"),
        max_connections: 4,
        min_connections: 0,
        acquire_timeout: Duration::from_secs(30),
    };
    let app = infrastructure::db::connect(&config)
        .await
        .expect("the API's URL with verify-full");
    infrastructure::run_migrations(&app).await.unwrap();
    let mut migrator = PgModuleMigrator::load(&app).await.unwrap();
    ModuleRegistry::new()
        .register(QualityModule::default())
        .run_migrations(&mut migrator)
        .unwrap();
    migrator.run(&app).await.unwrap();
    for i in 0..3 {
        sqlx::query(
            "INSERT INTO quality.jobs (tenant_id, kind, dataset) VALUES ($1::uuid, 'evaluate', $2)",
        )
        .bind(MAIN)
        .bind(format!("dataset-{i}"))
        .execute(&app)
        .await
        .unwrap();
    }
    World {
        _pg: pg,
        ca,
        ca_dir,
        port,
        app,
    }
}

impl World {
    async fn machine(&self, name: &str, state: &str) {
        sqlx::query("INSERT INTO machines (name, role, state) VALUES ($1, 'worker', $2)")
            .bind(name)
            .bind(state)
            .execute(&self.app)
            .await
            .unwrap();
    }

    async fn set_state(&self, name: &str, state: &str) {
        sqlx::query("UPDATE machines SET state = $2 WHERE name = $1 AND state <> 'retired'")
            .bind(name)
            .bind(state)
            .execute(&self.app)
            .await
            .unwrap();
    }

    /// The machine `name`'s agent files, and its connection options.
    fn files(&self, name: &str) -> (TempDir, sqlx::postgres::PgConnectOptions) {
        let dir = TempDir::new();
        write_machine(dir.path(), &self.ca.pem, &self.ca.machine(name));
        let url = format!("postgres://qd_queue@localhost:{}/postgres", self.port);
        let opts = options(&url, Some(&MachineFiles::new(dir.path()))).unwrap();
        (dir, opts)
    }

    /// The queue as the machine `name`'s worker opens it.
    async fn queue(&self, name: &str) -> (TempDir, PgPool, Queue) {
        let (dir, opts) = self.files(name);
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(opts)
            .await
            .expect("any worker's certificate opens the queue");
        let queue = Queue::new(
            pool.clone(),
            format!("qd-worker/{name}/0"),
            Timing::default(),
        );
        (dir, pool, queue)
    }
}

async fn visible_jobs(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM quality.jobs")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_queue_serves_approved_machines_and_lets_draining_ones_finish() {
    let w = world().await;
    w.machine("worker-ok", "approved").await;
    w.machine("worker-new", "pending").await;
    w.machine("worker-drain", "approved").await;
    w.machine("worker-gone", "retired").await;

    let (_d1, _p1, approved) = w.queue("worker-ok").await;
    assert!(approved.claim(&["evaluate"]).await.unwrap().is_some());

    let (_d2, pending_pool, pending) = w.queue("worker-new").await;
    let refused = pending.claim(&["evaluate"]).await.unwrap_err().to_string();
    assert!(refused.contains("worker-new is pending"), "{refused}");
    assert_eq!(
        visible_jobs(&pending_pool).await,
        0,
        "not through the table"
    );

    let (_d3, drain_pool, draining) = w.queue("worker-drain").await;
    let held = draining.claim(&["evaluate"]).await.unwrap().unwrap();
    w.set_state("worker-drain", "draining").await;
    let refused = draining.claim(&["evaluate"]).await.unwrap_err().to_string();
    assert!(refused.contains("worker-drain is draining"), "{refused}");
    assert_eq!(visible_jobs(&drain_pool).await, 3, "it may still work");
    assert!(draining.renew(&held).await.unwrap(), "renews its job");
    assert!(draining.finish(&held, None).await.unwrap(), "finishes it");

    for name in ["worker-gone", "worker-never"] {
        let (_d, pool, queue) = w.queue(name).await;
        let refused = queue.claim(&["evaluate"]).await.unwrap_err().to_string();
        assert!(refused.contains("not a live worker's"), "{name}: {refused}");
        assert!(queue.renew(&held).await.is_err(), "{name}");
        assert_eq!(visible_jobs(&pool).await, 0, "{name}");
    }
    assert!(approved.claim(&["evaluate"]).await.unwrap().is_some());
}

#[tokio::test]
async fn retiring_ends_the_machines_open_sessions_and_the_api_needs_tls() {
    let w = world().await;
    w.machine("worker-ok", "approved").await;
    w.machine("worker-two", "approved").await;
    let (_d1, opts) = w.files("worker-ok");
    let mut open = PgConnection::connect_with(&opts).await.unwrap();
    let (_d2, two) = w.files("worker-two");
    let mut other = PgConnection::connect_with(&two).await.unwrap();

    w.set_state("worker-ok", "retired").await;
    let ended = PgMachineSessions::new(w.app.clone()).end("worker-ok").await;
    assert_eq!(ended.unwrap(), 1);
    let gone = sqlx::query("SELECT 1").execute(&mut open).await;
    assert!(gone.is_err(), "its open session was ended");
    sqlx::query("SELECT 1").execute(&mut other).await.unwrap();

    // Its certificate still opens a session until it expires, which the
    // queue refuses.
    let (_d3, _pool, retired) = w.queue("worker-ok").await;
    assert!(retired.claim(&["evaluate"]).await.is_err());

    let plain = app_url(w.port, &w.ca_dir.path().join("ca.crt"), "disable");
    let refused = PgConnection::connect(&plain).await.unwrap_err().to_string();
    assert!(refused.contains("no encryption"), "{refused}");
}
