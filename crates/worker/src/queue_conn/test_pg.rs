//! A throwaway PostgreSQL 16 (the installation's image) as
//! `infra/postgres/tls.yml` makes it: TLS with a server certificate of the
//! test's CA, that CA as `ssl_ca_file`, the repository's `pg_hba.conf` and
//! `pg_ident.conf`, and its init scripts with certificates
//! (`QD_QUEUE_AUTH=certificate`): the application's role `app`, with its
//! password over TLS, and `qd_queue` without one. No rows at all.

use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, CopyTargetOptions, ImageExt};
use testcontainers_modules::postgres::Postgres;

use crate::testing::TestCa;

/// The application's role's password in the throwaway database.
pub(super) const APP_PASSWORD: &str = "app-test-password";

const HBA: &str = include_str!("../../../../infra/postgres/tls/pg_hba.conf");
const IDENT: &str = include_str!("../../../../infra/postgres/tls/pg_ident.conf");
const APP_ROLE: &str = include_str!("../../../../infra/postgres/initdb/10-app-role.sh");
const QUEUE_ROLE: &str = include_str!("../../../../infra/postgres/initdb/20-queue-role.sh");

/// Run after the repository's init scripts: TLS on, as tls.yml's command
/// does, and the repository's rules of who connects.
const TLS: &str = r#"
set -e
mkdir -p "$PGDATA/tls"
cp /tls-in/* "$PGDATA/tls/"
chmod 600 "$PGDATA/tls/server.key"
cp "$PGDATA/tls/pg_hba.conf" "$PGDATA/tls/pg_ident.conf" "$PGDATA/"
cat >> "$PGDATA/postgresql.conf" <<EOF
ssl = on
ssl_cert_file = '$PGDATA/tls/server.crt'
ssl_key_file = '$PGDATA/tls/server.key'
ssl_ca_file = '$PGDATA/tls/ca.crt'
EOF
"#;

/// The database and its port on this machine (`localhost`, the name in its
/// certificate).
pub(super) async fn tls_postgres(ca: &TestCa) -> (ContainerAsync<Postgres>, u16) {
    let server = ca.server();
    let file = |name: &str| CopyTargetOptions::new(format!("/tls-in/{name}")).with_mode(0o644);
    let init = |name: &str| {
        CopyTargetOptions::new(format!("/docker-entrypoint-initdb.d/{name}")).with_mode(0o755)
    };
    let container = Postgres::default()
        .with_copy_to(init("10-app-role.sh"), APP_ROLE.as_bytes().to_vec())
        .with_copy_to(init("20-queue-role.sh"), QUEUE_ROLE.as_bytes().to_vec())
        .with_copy_to(init("30-tls.sh"), TLS.as_bytes().to_vec())
        .with_copy_to(file("ca.crt"), ca.pem.clone().into_bytes())
        .with_copy_to(file("server.crt"), server.cert_pem.into_bytes())
        .with_copy_to(file("server.key"), server.key_pem.into_bytes())
        .with_copy_to(file("pg_hba.conf"), HBA.as_bytes().to_vec())
        .with_copy_to(file("pg_ident.conf"), IDENT.as_bytes().to_vec())
        .with_env_var("APP_DB_USER", "app")
        .with_env_var("APP_DB_PASSWORD", APP_PASSWORD)
        .with_env_var("QD_QUEUE_AUTH", "certificate")
        .with_tag("16-alpine")
        .start()
        .await
        .expect("a throwaway PostgreSQL: is Docker running?");
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    (container, port)
}
