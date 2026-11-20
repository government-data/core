//! A machine's sessions on the queue's PostgreSQL, ended when it is retired
//! (decision 0066, point 4): PostgreSQL checks a certificate only at the
//! handshake, so they would go on (`tools/verify/pg_client_cert.sh`).
//!
//! A session is the machine's when its client certificate's subject, as
//! `pg_stat_ssl` writes it (`/O=qd/OU=<role>/CN=<name>`), ends in its name.
//! Seeing another role's certificate takes `pg_read_all_stats`, and ending
//! its session `pg_signal_backend`: the application's role is given both by
//! `infra/postgres/initdb/20-queue-role.sh` when machines have certificates.
//! Without them nothing could be seen, which is reported as a failure and
//! not as no session.

use async_trait::async_trait;
use auth_gateway::domain::machine::is_label;
use auth_gateway::MachineSessions;
use domain::RepositoryError;
use sqlx::PgPool;

const MAY_END: &str = "SELECT pg_catalog.pg_has_role(current_user, 'pg_read_all_stats', 'USAGE') \
     AND pg_catalog.pg_has_role(current_user, 'pg_signal_backend', 'USAGE')";

const END: &str = "SELECT count(*) FROM ( \
       SELECT pg_catalog.pg_terminate_backend(s.pid) AS ended FROM pg_catalog.pg_stat_ssl s \
        WHERE s.pid <> pg_catalog.pg_backend_pid() AND s.client_dn LIKE '/O=qd/OU=%' \
          AND pg_catalog.substring(s.client_dn, '/CN=([a-z0-9-]+)$') = $1 \
     ) t WHERE t.ended";

/// A Postgres-backed [`MachineSessions`], as the application's role.
#[derive(Debug, Clone)]
pub struct PgMachineSessions {
    pool: PgPool,
}

impl PgMachineSessions {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MachineSessions for PgMachineSessions {
    async fn end(&self, name: &str) -> Result<u64, RepositoryError> {
        if !is_label(name) {
            return Ok(0);
        }
        let backend = |e: sqlx::Error| RepositoryError::Backend(e.to_string());
        let may: bool = sqlx::query_scalar(MAY_END)
            .fetch_one(&self.pool)
            .await
            .map_err(backend)?;
        if !may {
            return Err(RepositoryError::Backend(
                "the application's role lacks pg_read_all_stats or pg_signal_backend".into(),
            ));
        }
        let ended: i64 = sqlx::query_scalar(END)
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .map_err(backend)?;
        Ok(u64::try_from(ended).unwrap_or_default())
    }
}
