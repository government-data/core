//! Trino as one job sees it (decision 0062): the coordinator, and the
//! job's own token, which its details carry. A statement starts with a
//! token that has at least five minutes left; with less, the worker asks
//! the API for a new one first. A statement already running is not cut by
//! its token's expiry: Trino authenticates the submission only. There is
//! no other token: this worker's own reads nothing in Trino.
//!
//! An extraction's user is its dataset, and Trino reads its rules from the
//! API every ten seconds (point 7): a dataset just mapped may be refused
//! until then, so a statement refused by the rules waits and is tried once
//! more, once per job.
//!
//! Its statements carry Trino's client tags (decision 0026, point 1):
//! `job:<id>`, and `run:<cut id>` for a job of a cut, so Trino's UI and
//! `/v1/query/{id}` say whose a query is; each one's query id is logged
//! in the job's span.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use quality::job_span::cut_of;
use quality::job_trino::TrinoAccess;
use quality::queue::ClaimedJob;
use quality::worker_api::JobDetails;
use serde_json::Value;
use tokio::sync::Mutex;
use trino_client::{TrinoClient, TrinoError};
use uuid::Uuid;

use crate::api::Api;
use crate::evaluate::{failed, Failed};

/// What a statement must have left of its token when it starts.
const MARGIN: Duration = Duration::from_secs(5 * 60);

/// How long a statement refused by Trino's rules waits for its second try:
/// Trino's `security.refresh-period` (10 s, infra/trino) and two more.
const RULES_WAIT: Duration = Duration::from_secs(12);

/// Trino's error name for what its access control refuses.
const REFUSED: &str = "PERMISSION_DENIED";

type Rows = Vec<Vec<Value>>;

/// `run`, and once more after `wait` if Trino's rules refused it and
/// nothing `waited` yet. A second refusal, and any other error, is the
/// answer.
async fn once_more_if_refused<F, Fut>(
    waited: &AtomicBool,
    wait: Duration,
    run: F,
) -> Result<Rows, TrinoError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<Rows, TrinoError>>,
{
    match run().await {
        Err(TrinoError::Failed { name, .. })
            if name == REFUSED && !waited.swap(true, Ordering::SeqCst) =>
        {
            tracing::info!(
                seconds = wait.as_secs(),
                "refused by Trino's rules: waiting for them to be read again"
            );
            tokio::time::sleep(wait).await;
            run().await
        }
        other => other,
    }
}

pub(crate) struct JobTrino<'a> {
    api: &'a Api,
    /// The worker's client, tagged with the job's ids.
    client: TrinoClient,
    tenant: Uuid,
    id: Uuid,
    /// The token, and from when to ask for a new one.
    token: Mutex<(String, Instant)>,
    /// Whether a refused statement already waited for the rules.
    waited: AtomicBool,
}

/// The client tags of `job`'s statements: its id, and its cut's.
fn tags(job: &ClaimedJob) -> Vec<(&'static str, String)> {
    let cut = cut_of(&job.parameters).0;
    std::iter::once(("job", job.id.to_string()))
        .chain(cut.map(|cut| ("run", cut.to_string())))
        .collect()
}

/// When a token that lasts `expires_in` seconds from now is to be renewed.
fn renew_at(expires_in: u64) -> Instant {
    Instant::now() + Duration::from_secs(expires_in).saturating_sub(MARGIN)
}

impl<'a> JobTrino<'a> {
    /// The job's Trino if its source is read through Trino (decision 0048).
    pub(crate) fn of_job(
        api: &'a Api,
        job: &ClaimedJob,
        details: &JobDetails,
    ) -> Result<Option<Self>, Failed> {
        let on_trino = details.source.as_ref().is_some_and(|s| s.on_trino());
        on_trino.then(|| Self::new(api, job, details)).transpose()
    }

    /// `job`'s Trino: this worker's coordinator, tagged with the job's ids,
    /// and the token its details carry. Without either, the job cannot run.
    fn new(api: &'a Api, job: &ClaimedJob, details: &JobDetails) -> Result<Self, Failed> {
        let client = api.trino().ok_or_else(|| {
            failed(format!(
                "{}: this worker has no Trino (QD_TRINO_URL)",
                details.kind
            ))
        })?;
        let TrinoAccess { token, expires_in } = details.trino.clone().ok_or_else(|| {
            failed(format!(
                "{}: the job came with no Trino token",
                details.kind
            ))
        })?;
        Ok(Self {
            api,
            client: client.tagged(&tags(job)),
            tenant: job.tenant_id,
            id: job.id,
            token: Mutex::new((token, renew_at(expires_in))),
            waited: AtomicBool::new(false),
        })
    }

    /// Runs `sql` with the job's token: every row of every page.
    pub(crate) async fn execute(&self, sql: &str) -> Result<Rows, TrinoError> {
        let token = self.token().await?;
        self.client.execute(&token, sql).await
    }

    /// As [`Self::execute`], for a job whose user Trino's rules may not
    /// name yet (an extraction): refused, it waits and tries once more.
    pub(crate) async fn execute_when_allowed(&self, sql: &str) -> Result<Rows, TrinoError> {
        once_more_if_refused(&self.waited, RULES_WAIT, || self.execute(sql)).await
    }

    /// A token with at least [`MARGIN`] left, asked again if needed.
    async fn token(&self) -> Result<String, TrinoError> {
        let mut held = self.token.lock().await;
        if Instant::now() >= held.1 {
            let again = self
                .api
                .trino_again(self.tenant, self.id)
                .await
                .map_err(|e| TrinoError::Unavailable(format!("the job's token: {e}")))?;
            *held = (again.token, renew_at(again.expires_in));
        }
        Ok(held.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(kind: &str, parameters: Value) -> ClaimedJob {
        ClaimedJob {
            tenant_id: Uuid::nil(),
            id: Uuid::from_u128(0xa),
            kind: kind.into(),
            dataset: Some("orders".into()),
            parameters,
            engine: "trino".into(),
            attempts: 1,
        }
    }

    #[test]
    fn a_cuts_job_is_tagged_with_its_run_and_any_other_with_its_id_only() {
        let id = "00000000-0000-0000-0000-00000000000a".to_string();
        let cut = "00000000-0000-4000-8000-00000000000b";
        let extract = job("extract", serde_json::json!({ "cut_id": cut, "run": 2 }));
        assert_eq!(
            tags(&extract),
            [("job", id.clone()), ("run", cut.to_string())]
        );
        let map = job("map", serde_json::json!({ "source": "alpha" }));
        assert_eq!(tags(&map), [("job", id)]);
    }

    #[test]
    fn a_token_is_renewed_five_minutes_before_it_expires() {
        let now = Instant::now();
        let at = renew_at(15 * 60);
        assert!(at >= now + Duration::from_secs(10 * 60));
        assert!(at <= Instant::now() + Duration::from_secs(10 * 60));
        // One with less than five minutes left is renewed at once.
        assert!(renew_at(60) <= Instant::now());
    }

    fn refused() -> TrinoError {
        TrinoError::Failed {
            name: REFUSED.into(),
            message: "Access Denied: Cannot access catalog src_alpha".into(),
        }
    }

    /// Runs with answers in order, counting the tries; the wait it took.
    async fn tried(
        waited: &AtomicBool,
        answers: Vec<Result<Rows, TrinoError>>,
    ) -> (Result<Rows, TrinoError>, usize, Duration) {
        let answers = std::sync::Mutex::new(answers.into_iter());
        let tries = std::sync::atomic::AtomicUsize::new(0);
        let started = Instant::now();
        let result = once_more_if_refused(waited, Duration::from_millis(60), || async {
            tries.fetch_add(1, Ordering::SeqCst);
            answers.lock().unwrap().next().unwrap()
        })
        .await;
        (result, tries.load(Ordering::SeqCst), started.elapsed())
    }

    #[tokio::test]
    async fn a_statement_refused_by_the_rules_waits_and_is_tried_once_more() {
        let waited = AtomicBool::new(false);
        let rows = vec![vec![Value::from(3)]];
        let (result, tries, took) = tried(&waited, vec![Err(refused()), Ok(rows.clone())]).await;
        assert_eq!((result, tries), (Ok(rows), 2));
        assert!(took >= Duration::from_millis(60));
        // Once per job: refused again later, it is the answer, at once.
        let (result, tries, took) = tried(&waited, vec![Err(refused())]).await;
        assert_eq!((result, tries), (Err(refused()), 1));
        assert!(took < Duration::from_millis(60));
    }

    #[tokio::test]
    async fn refused_twice_or_failed_otherwise_it_is_not_tried_again() {
        let waited = AtomicBool::new(false);
        let (result, tries, _) = tried(&waited, vec![Err(refused()), Err(refused())]).await;
        assert_eq!((result, tries), (Err(refused()), 2));
        for other in [
            TrinoError::Refused(401),
            TrinoError::Unavailable("HTTP 500".into()),
            TrinoError::Failed {
                name: "TABLE_NOT_FOUND".into(),
                message: String::new(),
            },
        ] {
            let waited = AtomicBool::new(false);
            let (result, tries, took) = tried(&waited, vec![Err(other.clone())]).await;
            assert_eq!((result, tries), (Err(other), 1));
            assert!(took < Duration::from_millis(60) && !waited.load(Ordering::SeqCst));
        }
        let waited = AtomicBool::new(false);
        let (result, tries, _) = tried(&waited, vec![Ok(Vec::new())]).await;
        assert_eq!((result, tries), (Ok(Vec::new()), 1));
    }
}
