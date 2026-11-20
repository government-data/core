//! What every HTTP service of the backend shares: the JSON error envelope
//! ([`error::ApiError`]), the request id threaded through logs, responses and
//! errors ([`telemetry`]), the in-memory attempt limiter ([`rate_limit`]), and
//! the requests counted by route template ([`metrics`], decision 0071).
//! The `api` crate and the `auth-gateway`'s HTTP layer both build on it, so a
//! client sees the same errors from either.

pub mod error;
pub mod metrics;
pub mod rate_limit;
pub mod telemetry;
