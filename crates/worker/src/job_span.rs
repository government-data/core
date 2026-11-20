//! Each job runs in a span (decision 0026, point 1), so every line logged
//! while it runs, the libraries' it calls included, carries its ids at the
//! top level: `job_id`, `tenant`; `run_id`, the cut's id, and `run`, its
//! replacement number, only for a job of a cut (`extract`, `evaluate`; a
//! job queued before `run` was given carries only `run_id`); `kind`,
//! `dataset` (absent for a job of the whole warehouse) and `attempt`. The
//! API's job routes open a span with the same names.
//!
//! The span follows the job's future (`Instrument`), the lease's renewal
//! and the work moved off the runtime's threads ([`blocking`]).

use quality::job_span::cut_of;
use quality::queue::ClaimedJob;
use tracing::field::Empty;
use tracing::Span;

/// The span of `job`, from what it was claimed with.
pub(crate) fn span(job: &ClaimedJob) -> Span {
    let span = tracing::info_span!(
        "job",
        job_id = %job.id,
        tenant = %job.tenant_id,
        run_id = Empty,
        run = Empty,
        kind = %job.kind,
        dataset = Empty,
        attempt = job.attempts,
    );
    // Only the cut's id and number: never another value of the parameters.
    let (cut, run) = cut_of(&job.parameters);
    if let Some(cut) = cut {
        span.record("run_id", tracing::field::display(cut));
    }
    if let Some(run) = run {
        span.record("run", run);
    }
    if let Some(dataset) = job.dataset.as_deref() {
        span.record("dataset", dataset);
    }
    span
}

/// `f` on the blocking threads (DuckDB blocks), in the current span and
/// with the current subscriber, as `spawn_blocking` keeps neither.
pub(crate) async fn blocking<T, F>(f: F) -> Result<T, tokio::task::JoinError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let span = Span::current();
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    tokio::task::spawn_blocking(move || {
        tracing::dispatcher::with_default(&dispatch, || span.in_scope(f))
    })
    .await
}

#[cfg(test)]
#[path = "job_span/tests.rs"]
mod tests;
