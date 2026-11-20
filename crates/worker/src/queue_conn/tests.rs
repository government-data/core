//! The queue's options, and the queue opened for real with the machine's
//! certificate: a throwaway PostgreSQL 16 with the repository's TLS rules
//! (`test_pg`: `hostssl … cert` and the map from `OU=worker` to
//! `qd_queue`), and the agent's files made as the agent makes them (P-256,
//! the key in PKCS#8 PEM). No rows at all.

use sqlx::postgres::{PgPoolOptions, PgSslMode};
use sqlx::{Connection, PgConnection};

use super::test_pg::tls_postgres;
use super::{has_password, options};
use crate::machine::MachineFiles;
use crate::testing::{write_machine, TempDir, TestCa};

#[test]
fn with_a_secret_the_url_is_as_it_is() {
    let url = "postgres://qd_queue:pw@127.0.0.1:5432/knime?sslmode=disable";
    let o = options(url, None).unwrap();
    assert!(matches!(o.get_ssl_mode(), PgSslMode::Disable));
    assert_eq!(o.get_username(), "qd_queue");
}

#[test]
fn with_a_certificate_it_is_verify_full_and_no_password() {
    let files = MachineFiles::new("/var/lib/qd-node");
    let o = options(
        "postgres://qd_queue@db.internal:5432/quality?sslmode=disable",
        Some(&files),
    )
    .unwrap();
    assert!(matches!(o.get_ssl_mode(), PgSslMode::VerifyFull));
    assert_eq!(
        (o.get_host(), o.get_port(), o.get_database()),
        ("db.internal", 5432, Some("quality"))
    );
    for url in [
        "postgres://qd_queue:pw@db.internal/quality",
        "postgres://qd_queue@db.internal/quality?password=pw",
    ] {
        let refused = options(url, Some(&files)).unwrap_err().to_string();
        assert!(
            refused.contains("no password") && !refused.contains("pw@"),
            "{refused}"
        );
    }
    assert!(!has_password(
        "postgres://qd_queue@db.internal/quality?sslmode=require"
    ));
    assert!(!has_password("postgres://db.internal/q"));
}

/// The client certificate's serial and subject, as the server saw them.
async fn presented(conn: &mut PgConnection) -> (String, String) {
    sqlx::query_as(
        "SELECT client_serial::text, client_dn FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
    )
    .fetch_one(conn)
    .await
    .unwrap()
}

#[tokio::test]
async fn the_queue_is_opened_with_the_machines_certificate_and_takes_a_renewed_one() {
    let (ca, dir) = (TestCa::new(), TempDir::new());
    write_machine(dir.path(), &ca.pem, &ca.machine("worker-01"));
    let (_pg, port) = tls_postgres(&ca).await;
    let files = MachineFiles::new(dir.path());
    let url = format!("postgres://qd_queue@localhost:{port}/postgres");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options(&url, Some(&files)).unwrap())
        .await
        .expect("the queue with the machine's certificate");
    let mut first = pool.acquire().await.unwrap();
    let user: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&mut *first)
        .await
        .unwrap();
    assert_eq!(user, "qd_queue", "OU=worker mapped to the queue's role");
    let (old_serial, dn) = presented(&mut first).await;
    // pg_stat_ssl writes it its own way; the map read it as CN=…,OU=worker,O=qd.
    assert_eq!(dn, "/O=qd/OU=worker/CN=worker-01");

    // The agent renews: the next connection presents the new certificate,
    // with the same options; the one open goes on.
    write_machine(dir.path(), &ca.pem, &ca.machine("worker-01"));
    let mut second = pool.acquire().await.unwrap();
    let (new_serial, _) = presented(&mut second).await;
    assert_ne!(
        new_serial, old_serial,
        "a new connection reads the new files"
    );
    assert_eq!(presented(&mut first).await.0, old_serial);

    // Another role's certificate, or none, is refused: the application VM's
    // (`OU=app`, 0066 point 15) too, as it gets no jobs.
    for other in [ca.server(), ca.machine_of("app", "qd-app")] {
        write_machine(dir.path(), &ca.pem, &other);
        let opts = options(&url, Some(&files)).unwrap();
        assert!(
            PgConnection::connect_with(&opts).await.is_err(),
            "not OU=worker"
        );
    }
    let none = format!(
        "{url}?sslmode=verify-full&sslrootcert={}",
        files.ca().display()
    );
    assert!(
        PgConnection::connect(&none).await.is_err(),
        "no certificate"
    );
}
