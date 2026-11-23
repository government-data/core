//! The span a worker's call to a job's route runs in (decision 0026, point
//! 1), with the names the worker's own span uses, so the API's lines about
//! a job carry its ids at the top level as the worker's do: `job_id` and
//! `tenant` from the route's path; `kind`, `dataset` and, for a job of a
//! cut, `run_id` (the cut's id) and `run` (its replacement number) once the
//! job is loaded ([`record`]). Only ids: never another value of the
//! parameters.

use axum::extract::rejection::RawPathParamsRejection;
use axum::extract::{RawPathParams, Request};
use axum::middleware::Next;
use axum::response::Response;
use tracing::field::Empty;
use tracing::{Instrument, Span};
use uuid::Uuid;

/// A job's cut, as its parameters name it: its id (`cut_id`, the logs'
/// `run_id`) and its replacement number (`run`), each if it is there and
/// well formed. Jobs queued before `run` was added carry only the id.
pub fn cut_of(parameters: &serde_json::Value) -> (Option<Uuid>, Option<i64>) {
    let id = parameters
        .get("cut_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|s| s.parse::<Uuid>().ok());
    let run = parameters.get("run").and_then(serde_json::Value::as_i64);
    (id, run)
}

/// Middleware of `/internal/jobs/{id}/…` (and of `/internal/jobs/{tenant}/{id}/…`,
/// whose tenant it records too): the rest of the request runs inside the
/// job's span. A path that does not parse is left to the handler, which
/// refuses it as before.
pub(crate) async fn enter(
    params: Result<RawPathParams, RawPathParamsRejection>,
    request: Request,
    next: Next,
) -> Response {
    let Some((tenant, id)) = params.ok().and_then(|p| ids(&p)) else {
        return next.run(request).await;
    };
    let span = tracing::info_span!(
        "job",
        job_id = %id,
        tenant = Empty,
        run_id = Empty,
        run = Empty,
        kind = Empty,
        dataset = Empty,
    );
    if let Some(tenant) = tenant {
        span.record("tenant", tracing::field::display(tenant));
    }
    next.run(request).instrument(span).await
}

/// The job's id and, on a tenant-qualified route, its tenant: `None` if
/// one of them is not an id.
fn ids(params: &RawPathParams) -> Option<(Option<Uuid>, Uuid)> {
    let mut tenant = None;
    let mut id = None;
    for (name, value) in params {
        let value = value.parse::<Uuid>().ok()?;
        match name {
            "tenant" => tenant = Some(value),
            "id" => id = Some(value),
            _ => {}
        }
    }
    Some((tenant, id?))
}

/// Records on the current job span what the loaded job adds to its path.
/// Outside one it records nothing.
pub(crate) fn record(kind: &str, dataset: Option<&str>, parameters: &serde_json::Value) {
    let span = Span::current();
    span.record("kind", kind);
    if let Some(dataset) = dataset {
        span.record("dataset", dataset);
    }
    let (cut, run) = cut_of(parameters);
    if let Some(cut) = cut {
        span.record("run_id", tracing::field::display(cut));
    }
    if let Some(run) = run {
        span.record("run", run);
    }
}

#[cfg(test)]
#[path = "job_span/tests.rs"]
mod tests;
