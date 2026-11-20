//! How the worker reaches the queue (decisions 0023, 0066 point 7). With a
//! secret, `QD_QUEUE_DATABASE_URL` as it is, the `qd_queue` password in it
//! (development). With the machine's certificate, the same URL without a
//! password, and TLS added here: `verify-full` against `ca.crt`, the
//! machine's certificate and key as the client's (PostgreSQL maps
//! `OU=worker` to `qd_queue`; `tools/verify/pg_client_cert.sh`).
//!
//! A renewed certificate needs nothing here: sqlx 0.8 reads the files given
//! as paths at each new connection's handshake
//! (`sqlx-core-0.8.6/src/net/tls/tls_rustls.rs`, `handshake`, lines 114,
//! 115 and 146, through `CertificateInput::data`, `net/tls/mod.rs` line
//! 46). PostgreSQL checks a certificate at the handshake only, so open
//! connections go on. Both the certificate and the key are read from
//! `machine.pem`, the one file the agent renames whole: `machine.crt` and
//! `machine.key` are renamed one after the other, and a connection between
//! the two would read a key that is not its certificate's.

use sqlx::postgres::{PgConnectOptions, PgSslMode};

use crate::machine::MachineFiles;

type Failure = Box<dyn std::error::Error + Send + Sync>;

/// The queue's connection options: `url` as it is, or, with the machine's
/// `files`, `url` (which must hold no password) with its certificate.
pub(crate) fn options(
    url: &str,
    files: Option<&MachineFiles>,
) -> Result<PgConnectOptions, Failure> {
    let Some(files) = files else {
        return Ok(url.parse()?);
    };
    if has_password(url) {
        return Err(
            "QD_QUEUE_DATABASE_URL must hold no password with QD_MACHINE_DIR: \
                    the queue is opened with the machine's certificate"
                .into(),
        );
    }
    let options: PgConnectOptions = url.parse()?;
    Ok(options
        .ssl_mode(PgSslMode::VerifyFull)
        .ssl_root_cert(files.ca())
        .ssl_client_cert(files.bundle())
        .ssl_client_key(files.bundle()))
}

/// Whether a `postgres://user:password@…` URL, or its query, names a
/// password.
fn has_password(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    let in_user = authority
        .rsplit_once('@')
        .is_some_and(|(user, _)| user.contains(':'));
    let in_query = url
        .split_once('?')
        .is_some_and(|(_, q)| q.split('&').any(|p| p.starts_with("password=")));
    in_user || in_query
}

#[cfg(test)]
#[path = "queue_conn/test_pg.rs"]
mod test_pg;

#[cfg(test)]
#[path = "queue_conn/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "queue_conn/machines_tests.rs"]
mod machines_tests;
