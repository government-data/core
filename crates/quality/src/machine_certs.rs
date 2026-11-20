//! Which machine calls, by the client certificate Traefik's internal entry
//! point verified (decision 0066, point 5). A module never reads a
//! certificate nor the machines' registry: `server` implements this port
//! with the same reading and check as the machines' own routes, as
//! [`crate::worker_api::MachineTokens`] does for a worker's token.

use async_trait::async_trait;
use axum::http::HeaderMap;

/// The role of Trino's coordinator, as its certificate's `OU` says it.
pub const TRINO_COORDINATOR: &str = "trino-coordinator";

/// Port: the machine whose certificate came with a request.
#[async_trait]
pub trait MachineCertificates: Send + Sync {
    /// The name of the machine of `role` whose certificate came with
    /// `headers`, if the installation accepts it (not retired, the serial in
    /// use or the next one) and it may work (approved or draining). `None`
    /// for anything else, a certificate missing or unreadable included.
    async fn machine(&self, headers: &HeaderMap, role: &str) -> Option<String>;
}
