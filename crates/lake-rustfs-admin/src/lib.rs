//! The lake admin adapter of `lake-rustfs` (decision 0078, point 8): it
//! serves `lake-admin/1` to the core's server alone, with mTLS of the
//! installation's CA both ways, and keeps no secret or state of its own
//! (point 9). Built in the core's `crates/` until it moves to the module's
//! repository (step 15); never a dependency of `server`.

use std::sync::Arc;

use tokio::net::TcpListener;

pub mod policy;
pub mod routes;
pub mod tls;

pub use tls::Certs;

/// Serves `lake-admin/1` on `tcp` until `shutdown` resolves.
pub async fn serve(
    tcp: TcpListener,
    certs: Arc<Certs>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let listener = tls::MtlsListener::new(tcp, certs)?;
    let app = routes::router().into_make_service_with_connect_info::<tls::Peer>();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
}
