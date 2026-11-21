//! One job a slot took: run it, renewing its lease meanwhile, and finish
//! it, saying how it ended for the metrics (decision 0071).

use quality::queue::{ClaimedJob, Queue, Timing};
use quality::worker_metrics::JobResult;
use tracing::Instrument;

use crate::api;

/// Runs a job it holds, renewing its lease meanwhile, and finishes it; says
/// how it ended, for the metrics.
pub(crate) async fn one_job(
    queue: &Queue,
    api: &api::Api,
    ca: Option<&str>,
    job: &ClaimedJob,
) -> JobResult {
    tracing::info!("job taken");
    let renewing = {
        let (queue, job) = (queue.clone(), job.clone());
        let renew = async move {
            let mut every = tokio::time::interval(Timing::default().lease / 3);
            every.tick().await;
            loop {
                every.tick().await;
                if !matches!(queue.renew(&job).await, Ok(true)) {
                    tracing::warn!("lease lost");
                    break;
                }
            }
        };
        tokio::spawn(renew.in_current_span())
    };
    let outcome = crate::run_job(api, ca, job).await;
    renewing.abort();
    let error = outcome.as_ref().err().map(ToString::to_string);
    let not_now = outcome.as_ref().is_err_and(|e| e.is::<api::NotNow>());
    result(
        queue.finish(job, error.as_deref()).await,
        error.as_deref(),
        not_now,
    )
}

/// How a job ended, for the metrics, from what finishing it gave
/// (`Ok(true)`: finished as this worker's), the job's error and whether
/// that error was the API's "not now" (503).
fn result<E: std::fmt::Display>(
    finished: Result<bool, E>,
    error: Option<&str>,
    not_now: bool,
) -> JobResult {
    match finished {
        Ok(true) => match error {
            None => {
                tracing::info!("job done");
                JobResult::Ok
            }
            Some(e) => {
                tracing::warn!("job failed: {e}");
                JobResult::Failed
            }
        },
        // The API put it back (503): nothing failed.
        Ok(false) if not_now => {
            tracing::info!("the job went back to the queue: {}", api::NotNow);
            JobResult::Cancelled
        }
        Ok(false) => {
            tracing::warn!("the job was no longer this worker's");
            JobResult::Cancelled
        }
        Err(e) => {
            tracing::warn!("finishing the job: {e}");
            JobResult::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{result, JobResult};

    type Finished = Result<bool, &'static str>;

    #[test]
    fn a_job_finished_without_error_is_ok() {
        assert_eq!(result(Finished::Ok(true), None, false), JobResult::Ok);
    }

    #[test]
    fn a_job_finished_with_its_error_failed() {
        assert_eq!(
            result(Finished::Ok(true), Some("boom"), false),
            JobResult::Failed
        );
    }

    #[test]
    fn a_job_that_could_not_be_finished_failed() {
        assert_eq!(
            result(Finished::Err("no database"), None, false),
            JobResult::Failed
        );
        assert_eq!(
            result(Finished::Err("no database"), Some("boom"), false),
            JobResult::Failed
        );
    }

    #[test]
    fn a_job_the_api_put_back_is_cancelled() {
        let not_now = Some("the API cannot give the job now (503)");
        assert_eq!(
            result(Finished::Ok(false), not_now, true),
            JobResult::Cancelled
        );
    }

    #[test]
    fn a_job_whose_lease_was_lost_is_cancelled() {
        assert_eq!(
            result(Finished::Ok(false), None, false),
            JobResult::Cancelled
        );
        assert_eq!(
            result(Finished::Ok(false), Some("boom"), false),
            JobResult::Cancelled
        );
    }
}
