//! A job's token for Trino on the workers' routes (decision 0062), over an
//! ephemeral PostgreSQL: an extraction of a source on Trino gets a token
//! naming its tenant, itself and its dataset, recorded first; it asks
//! again on `…/trino` and is recorded again; a job that is not the
//! caller's, or reads no source on Trino, gets none; without the audit log,
//! nothing is given. Synthetic names only.

pub mod worker;

use std::sync::atomic::Ordering;

use axum::http::StatusCode;
use quality::job_trino::TrinoAccess;
use quality::source::Source;
use quality::source_store;
use quality::worker_api::JobDetails;
use uuid::Uuid;
use worker::*;

/// The source `core` on `driver`.
async fn source_on(f: &Fixture, driver: &str) {
    tenancy::within(tenant(B), async {
        let source = Source {
            name: "core".into(),
            driver: driver.into(),
            fields: [("host", "db.example"), ("database", "core")]
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .into(),
            secrets: [("user", "reader"), ("password", "synthetic")]
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .into(),
        };
        source_store::put(&f.app, &Mirror, &source).await.unwrap();
    })
    .await;
}

/// A running extraction of `clientes` (read from `core`) held by `w1`.
async fn extraction(f: &Fixture) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO quality.jobs (tenant_id, kind, dataset, parameters, state, worker, lease_until)
         VALUES ($1::uuid, 'extract', 'clientes', '{\"cut\": \"2026-11-10\"}', 'running', 'w1/0',
                 now() + interval '1 minute') RETURNING id",
    )
    .bind(B)
    .fetch_one(&f.db.pool)
    .await
    .unwrap()
}

fn given(f: &Fixture) -> Vec<(Uuid, String, String)> {
    f.issued.given.lock().unwrap().clone()
}

#[tokio::test]
async fn an_extraction_on_trino_gets_its_own_token_recorded_first_and_asks_again() {
    let f = fixture().await;
    source_on(&f, "sqlserver").await;
    let job = extraction(&f).await;
    let uri = format!("/internal/jobs/{B}/{job}");
    let (status, body) = f.call("GET", &uri, "token-w1", None).await;
    assert_eq!(status, StatusCode::OK);
    let details: JobDetails = serde_json::from_slice(&body).unwrap();
    assert!(details.source.unwrap().on_trino() && details.lake.is_none());
    let trino = details
        .trino
        .expect("an extraction on Trino gets its token");
    let subject = format!("job:{B}:{job}:extract:clientes");
    assert_eq!(trino.token, format!("{subject}|w1"));
    assert_eq!(trino.expires_in, 900);
    let recorded = (
        job,
        "w1".to_string(),
        "trino/src_core+clientes_bronze".to_string(),
    );
    assert_eq!(given(&f), vec![recorded.clone()]);

    let (status, body) = f
        .call("GET", &format!("{uri}/trino"), "token-w1", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let again: TrinoAccess = serde_json::from_slice(&body).unwrap();
    assert_eq!(again.token, format!("{subject}|w1"));
    assert_eq!(given(&f), vec![recorded.clone(), recorded]);

    // Another machine's worker does not hold it.
    let other = f
        .call("GET", &format!("{uri}/trino"), "token-w2", None)
        .await;
    assert_eq!(other.0, StatusCode::NOT_FOUND);
    assert_eq!(given(&f).len(), 2);
}

#[tokio::test]
async fn a_job_reading_no_source_on_trino_gets_no_token() {
    let f = fixture().await;
    source_on(&f, "postgres").await;
    let job = extraction(&f).await;
    let uri = format!("/internal/jobs/{B}/{job}");
    let (status, body) = f.call("GET", &uri, "token-w1", None).await;
    assert_eq!(status, StatusCode::OK);
    let details: JobDetails = serde_json::from_slice(&body).unwrap();
    assert!(details.trino.is_none());
    assert!(given(&f).iter().all(|(_, _, r)| !r.starts_with("trino/")));
    let again = f
        .call("GET", &format!("{uri}/trino"), "token-w1", None)
        .await;
    assert_eq!(again.0, StatusCode::NOT_FOUND);

    let evaluation = f.job(B, "w1/0", 60).await;
    let none = f
        .call(
            "GET",
            &format!("/internal/jobs/{B}/{evaluation}/trino"),
            "token-w1",
            None,
        )
        .await;
    assert_eq!(none.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_token_that_cannot_be_recorded_is_not_given() {
    let f = fixture().await;
    source_on(&f, "sqlserver").await;
    let job = extraction(&f).await;
    f.issued.down.store(true, Ordering::SeqCst);
    let uri = format!("/internal/jobs/{B}/{job}");
    let (status, _) = f.call("GET", &uri, "token-w1", None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, _) = f
        .call("GET", &format!("{uri}/trino"), "token-w1", None)
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

/// A running extraction of `clientes`, as [`extraction`], marked `uses`
/// Trino when queued, on its first attempt.
async fn marked(f: &Fixture, uses: Option<bool>) -> Uuid {
    let job = extraction(f).await;
    sqlx::query("UPDATE quality.jobs SET uses_trino = $2, attempts = 1 WHERE id = $1")
        .bind(job)
        .bind(uses)
        .execute(&f.db.pool)
        .await
        .unwrap();
    job
}

/// The job's state, attempts, worker and mark.
async fn row(f: &Fixture, job: Uuid) -> (String, i32, Option<String>, Option<bool>) {
    sqlx::query_as("SELECT state, attempts, worker, uses_trino FROM quality.jobs WHERE id = $1")
        .bind(job)
        .fetch_one(&f.db.pool)
        .await
        .unwrap()
}

/// The job's details, as its worker `w1` asks them: their status.
async fn details(f: &Fixture, job: Uuid) -> StatusCode {
    let uri = format!("/internal/jobs/{B}/{job}");
    f.call("GET", &uri, "token-w1", None).await.0
}

#[tokio::test]
async fn its_details_correct_what_a_job_was_marked_either_way() {
    let f = fixture().await;
    source_on(&f, "sqlserver").await;
    // Marked as not using Trino: its mapping read another source then.
    let job = marked(&f, Some(false)).await;
    assert_eq!(details(&f, job).await, StatusCode::OK);
    assert_eq!(row(&f, job).await.3, Some(true), "counted from now on");

    let f = fixture().await;
    source_on(&f, "postgres").await;
    for uses in [None, Some(true)] {
        let job = marked(&f, uses).await;
        assert_eq!(details(&f, job).await, StatusCode::OK);
        assert_eq!(row(&f, job).await.3, Some(false), "no longer busy");
    }
}

#[tokio::test]
async fn a_job_found_on_trino_while_it_is_held_goes_back_to_the_queue() {
    let f = fixture().await;
    source_on(&f, "sqlserver").await;
    // Taken as known not to use Trino while Trino was held.
    let job = marked(&f, Some(false)).await;
    sqlx::query(
        "INSERT INTO quality.trino_holds (tenant_id, machine, expires_at)
         VALUES ($1::uuid, 'trino-1', now() + interval '1 minute')",
    )
    .bind(B)
    .execute(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(details(&f, job).await, StatusCode::SERVICE_UNAVAILABLE);
    // Queued again, its attempt given back, held now; nothing given.
    assert_eq!(row(&f, job).await, ("queued".into(), 0, None, Some(true)));
    assert!(given(&f).is_empty());
    // Another tenant's hold does not hold this one's.
    sqlx::query("UPDATE quality.trino_holds SET tenant_id = $1::uuid")
        .bind(A)
        .execute(&f.db.pool)
        .await
        .unwrap();
    let job = marked(&f, Some(false)).await;
    assert_eq!(details(&f, job).await, StatusCode::OK);
    assert_eq!(row(&f, job).await.0, "running");
}
