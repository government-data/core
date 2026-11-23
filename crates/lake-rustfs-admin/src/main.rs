//! `lake-rustfs-admin`: the lake admin adapter (decision 0078, point 8),
//! one per lake, on the lake's machines next to its gate (point 9).
//!
//! Configuration, from the environment:
//!
//! - `QD_LAKE_ADMIN_ADDR`: where it listens (`0.0.0.0:9020`);
//! - `QD_LAKE_ADMIN_TLS_DIR`: the node agent's directory (`/run/qd-node`),
//!   with `ca.crt` and `machine.pem` (the certificate then the key), read
//!   again whenever they change;
//! - `RUST_LOG`: the level (`info`). Lines are JSON; none holds a key, a
//!   secret, a policy or a body.
//!
//! It exits non-zero when it cannot listen or read its certificate.

use std::sync::Arc;

use lake_rustfs_admin::{serve, Certs};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let addr = std::env::var("QD_LAKE_ADMIN_ADDR").unwrap_or_else(|_| "0.0.0.0:9020".into());
    let dir = std::env::var("QD_LAKE_ADMIN_TLS_DIR").unwrap_or_else(|_| "/run/qd-node".into());
    let certs = Arc::new(Certs::from_dir(dir));
    if let Err(e) = certs.config() {
        tracing::error!(error = %e, "the adapter's certificate");
        return std::process::ExitCode::FAILURE;
    }
    let tcp = match tokio::net::TcpListener::bind(&addr).await {
        Ok(tcp) => tcp,
        Err(e) => {
            tracing::error!(%addr, error = %e, "listening");
            return std::process::ExitCode::FAILURE;
        }
    };
    tracing::info!(%addr, "serving lake-admin/1");
    match serve(tcp, certs, stopped()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "serving");
            std::process::ExitCode::FAILURE
        }
    }
}

/// `SIGTERM` (a container stopped) or `SIGINT`.
async fn stopped() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = term.recv() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
