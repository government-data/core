//! `qd-worker` (decision 0018): one loop per slot — take a job from the
//! queue, ask the API for its details (its credentials and its catalog come
//! with them), run it, report, finish — renewing the job's lease while it
//! runs. Workers pull: nothing pushes to them.
//!
//! It runs in one of two modes (`config.rs`):
//!
//! - **with the machine's certificate** (decision 0066, step 7), when
//!   `QD_MACHINE_DIR` names the node agent's directory (`qd-node`'s
//!   `QD_AGENT_DIR`): `machine.pem` is its identity and `ca.crt` the only
//!   root it trusts for the API, its token is asked with no secret, and
//!   the queue is opened with the certificate too. It holds no secret: with
//!   `QD_WORKER_CLIENT_SECRET` set as well it does not start. A renewed
//!   certificate is taken without a restart. Its slots are
//!   `<client id>/<machine>/<slot>`, the machine its certificate's `CN`;
//! - **with a secret**, without `QD_MACHINE_DIR`: development only, until
//!   the dev stack moves to certificates (step 8), when it goes. Its slots
//!   are `<client id>/<slot>`.
//!
//! Configuration, from the environment (secrets also from `<NAME>_FILE`):
//!
//! - `QD_QUEUE_DATABASE_URL`: PostgreSQL as the queue's role (`qd_queue`);
//!   with a certificate, without a password
//!   (`postgres://qd_queue@<host>:5432/<database>`), TLS `verify-full`
//!   added by the worker;
//! - `QD_API_URL`: the API, `https://<domain>/api`; with a certificate the
//!   internal entry point's, `https://<internal domain>:9443/api`;
//! - `QD_WORKER_CLIENT_ID`: its OIDC machine client, allowed the audience
//!   `quality-api` (with a certificate, the role's client, e.g.
//!   `qd-worker`, registered with `--machine-role worker`);
//! - `QD_MACHINE_DIR`, or `QD_WORKER_CLIENT_SECRET`: the mode, above;
//! - `QD_CA_FILE`: a CA to trust for the lake, and for the API
//!   with a secret (development);
//! - `QD_LAKE_URL`: the lake as this worker reaches it, when the lake
//!   takes only machines' certificates (decision 0066, point 10): the proxy
//!   on its loopback (`infra/worker/lake.yml`), which presents the
//!   machine's, `https://<machine name>:9000`. The worker waits for it, and
//!   reaches every job's lake there;
//! - `QD_WORKER_SLOTS`: jobs at a time (1);
//! - `QD_WORKER_MIN_FREE_BYTES` (5 GB) and `QD_WORKER_MIN_FREE_MEMORY_PCT`
//!   (10): below either no job is claimed (`guard.rs`, decision 0072);
//! - `QD_METRICS_DIR`: where its metrics file goes (`/run/qd-metrics`, `off`
//!   for none; decision 0071);
//! - `QD_DUCKDB_EXTENSIONS`: DuckDB's extension directory, which sources
//!   load from offline (`~/.duckdb/extensions`; decision 0077).
//!
//! `qd-worker --install-extensions` installs the lake's DuckDB extensions
//! and exits: the image runs it when it is built and adds the sources'
//! drivers from pinned files, so jobs fetch nothing.

mod api;
mod config;
mod distribution;
mod evaluate;
mod extract;
mod findings;
mod guard;
mod icd;
mod job_span;
mod lake_tables;
mod lease;
mod machine;
mod maintain;
mod map;
mod metrics;
mod query;
mod queue_conn;
mod records;
mod references;
mod stages;
mod tables;
mod test_rule;
#[cfg(test)]
mod testing;

use std::sync::Arc;
use std::time::Duration;

use quality::queue::{ClaimedJob, Queue, Timing};
use sqlx::postgres::PgPoolOptions;
use tracing::Instrument;

type Failure = Box<dyn std::error::Error + Send + Sync>;

/// How long a slot waits when the queue is empty.
const IDLE: Duration = Duration::from_secs(5);

/// How long a slot waits when the API gives it no token.
const NO_TOKEN: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() {
    logging::init("worker");
    if std::env::args().nth(1).as_deref() == Some("--install-extensions") {
        if let Err(e) = quality_rules_duckdb::Runner::install_extensions() {
            eprintln!("qd-worker: installing DuckDB's extensions: {e}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(e) = run().await {
        eprintln!("qd-worker: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Failure> {
    let mode = config::Mode::from_env()?;
    if let Some(files) = mode.files() {
        config::wait_for_join(files).await;
    }
    let ca = config::env("QD_CA_FILE");
    let mut http = reqwest::Client::builder().timeout(Duration::from_secs(30));
    if let Some(ca) = &ca {
        http = http.add_root_certificate(reqwest::Certificate::from_pem(&std::fs::read(ca)?)?);
    }
    let queue = queue_conn::options(&config::value("QD_QUEUE_DATABASE_URL")?, mode.files())?;
    let mut api = mode.api(http.build()?, &config::value("QD_API_URL")?)?;
    if let Some(url) = config::env("QD_LAKE_URL") {
        config::wait_for_lake(&url).await?;
        api = api.with_lake(url);
    }
    let api = Arc::new(api);
    let slots: usize = config::env("QD_WORKER_SLOTS")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let pool = PgPoolOptions::new()
        .max_connections(u32::try_from(slots).unwrap_or(u32::MAX).saturating_add(1))
        .connect_with(queue)
        .await?;
    let worker = api.worker();
    tracing::info!(slots, worker, "qd-worker started");
    let metrics = metrics::start(slots);
    let guard = Arc::new(guard::Guard::from_env(metrics.clone())?);
    let mut handles = Vec::new();
    for slot in 0..slots {
        let queue = Queue::new(pool.clone(), format!("{worker}/{slot}"), Timing::default());
        let (api, ca, metrics) = (api.clone(), ca.clone(), metrics.clone());
        let guard = guard.clone();
        handles.push(tokio::spawn(slot_loop(queue, api, ca, metrics, guard)));
    }
    for h in handles {
        h.await?;
    }
    Ok(())
}

/// Every kind of job the queue holds: a kind left out is never run.
const CLAIMED: &[&str] = &[
    "evaluate",
    "extract",
    "map",
    "maintain",
    "test_rule",
    "tables",
    "lake_tables",
    "records",
    "query",
];

async fn slot_loop(
    queue: Queue,
    api: Arc<api::Api>,
    ca: Option<String>,
    metrics: quality::worker_metrics::WorkerMetrics,
    guard: Arc<guard::Guard>,
) {
    loop {
        // No job is taken that could not be run: a machine not approved
        // yet, or retired, gets no token (decision 0066, point 6).
        if let Err(e) = api.ready().await {
            tracing::warn!("no token for the API, no job taken: {e}");
            tokio::time::sleep(NO_TOKEN).await;
            continue;
        }
        // At its own disk or memory limit it takes nothing (0072, point 14).
        let Some(claimed) = guard.claim(|| queue.claim(CLAIMED)).await else {
            tokio::time::sleep(IDLE).await;
            continue;
        };
        match claimed {
            Ok(Some(job)) => {
                let span = job_span::span(&job);
                let running = metrics.started(&job.kind);
                let result = lease::one_job(&queue, &api, ca.as_deref(), &job)
                    .instrument(span)
                    .await;
                metrics.finished(running, result);
            }
            Ok(None) => tokio::time::sleep(IDLE).await,
            Err(e) => {
                tracing::warn!("queue: {e}");
                tokio::time::sleep(IDLE).await;
            }
        }
    }
}

async fn run_job(api: &api::Api, ca: Option<&str>, job: &ClaimedJob) -> Result<(), Failure> {
    let details = match api.details(job.tenant_id, job.id).await {
        Ok(details) => details,
        Err(e) if e.is::<api::NotNow>() => return Err(e),
        Err(e) => return Err(format!("job details: {e}").into()),
    };
    let ca = ca.map(str::to_string);
    if details.kind == "extract" {
        return run_extract(api, job, details, ca).await;
    }
    if details.kind == "maintain" {
        let report = maintain::maintain(api.http(), details, ca).await?;
        tracing::info!(tables = report.tables.len(), "maintained");
        return send_report(api, job, &report).await;
    }
    if details.kind == "map" {
        let report = job_span::blocking(move || map::map(&details)).await??;
        tracing::info!(
            columns = report.profile.columns.len(),
            sampled = report.profile.sampled,
            "mapped"
        );
        return send_report(api, job, &report).await;
    }
    if details.kind == "test_rule" {
        let report =
            job_span::blocking(move || test_rule::test_rule(&details, ca.as_deref())).await??;
        // Counts only: the records read are the API's to mask and audit.
        tracing::info!(
            records = report.records,
            rules = report.rules.len(),
            "tested"
        );
        return send_report(api, job, &report).await;
    }
    if details.kind == "lake_tables" {
        let report =
            job_span::blocking(move || lake_tables::list(&details, ca.as_deref())).await??;
        tracing::info!(
            tables = report.tables.len(),
            truncated = report.truncated,
            "listed the lake"
        );
        return send_report(api, job, &report).await;
    }
    if details.kind == "records" {
        let page = job_span::blocking(move || records::read(&details)).await??;
        // Counts only: the values are the API's to mask and audit.
        tracing::info!(records = page.rows.len(), more = page.more, "read a page");
        return send_report(api, job, &page).await;
    }
    if details.kind == "query" {
        let result = job_span::blocking(move || query::query(&details, ca.as_deref())).await??;
        // Counts only: the rows and the engine's words are the API's to mask.
        tracing::info!(
            rows = result.rows.len(),
            more = result.more,
            failed = result.error.is_some(),
            "ran a statement"
        );
        return send_report(api, job, &result).await;
    }
    if details.kind == "tables" {
        let report = job_span::blocking(move || tables::list(&details)).await??;
        tracing::info!(
            tables = report.tables.len(),
            truncated = report.truncated,
            "listed"
        );
        return send_report(api, job, &report).await;
    }
    // DuckDB blocks: off the runtime's threads.
    let report = job_span::blocking(move || evaluate::evaluate(&details, ca.as_deref())).await??;
    tracing::info!(
        records = report.records,
        rules = report.rules.len(),
        "evaluated"
    );
    send_report(api, job, &report).await
}

/// A job's report to the API.
async fn send_report(
    api: &api::Api,
    job: &ClaimedJob,
    report: &impl serde::Serialize,
) -> Result<(), Failure> {
    api.report(job.tenant_id, job.id, report)
        .await
        .map_err(|e| format!("report: {e}").into())
}

/// An extraction: create its table if it is the first, ask again for that
/// table's files then, and replace the cut (decision 0037).
async fn run_extract(
    api: &api::Api,
    job: &ClaimedJob,
    details: quality::worker_api::JobDetails,
    ca: Option<String>,
) -> Result<(), Failure> {
    let details = Arc::new(details);
    let (d, c) = (details.clone(), ca.clone());
    let plan = job_span::blocking(move || extract::prepare(&d, c.as_deref())).await??;
    let lake = match (&details.lake, plan.created) {
        (Some(_), true) => api
            .lake_again(job.tenant_id, job.id)
            .await
            .map_err(|e| format!("lake credentials: {e}"))?,
        (Some(lake), false) => lake.clone(),
        (None, _) => return Err("extract: no lake access came with the job".into()),
    };
    let d = details.clone();
    let report =
        job_span::blocking(move || extract::write(&d, &plan, &lake, ca.as_deref())).await??;
    tracing::info!(
        rows = report.ranges.iter().map(|r| r.rows).sum::<i64>(),
        "extracted"
    );
    send_report(api, job, &report).await
}

#[cfg(test)]
mod tests {
    use super::CLAIMED;

    /// `quality::queue_ready::KINDS` is checked against the migrations'.
    #[test]
    fn the_worker_claims_every_kind_the_queue_holds() {
        let mut claimed = CLAIMED.to_vec();
        let mut allowed = quality::queue_ready::KINDS.to_vec();
        claimed.sort_unstable();
        allowed.sort_unstable();
        assert_eq!(claimed, allowed);
    }
}
