//! A source's tables on the workers' routes (decision 0042), over an
//! ephemeral PostgreSQL: a `tables` job gets its source with no table, its
//! report becomes the source's listing, a late report of an older job
//! never replaces a newer one, nothing of the system's schemas is taken,
//! and the listing goes with its source. Synthetic names only.

pub mod worker;

use axum::http::StatusCode;
use quality::source::Source;
use quality::source_store;
use quality::source_tables::{self, ListingReason, ListingState};
use quality::worker_api::JobDetails;
use serde_json::json;
use uuid::Uuid;
use worker::*;

async fn core(f: &Fixture) {
    source_on(f, "postgres").await;
}

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

/// A running listing of `core`, queued `ago` seconds ago.
async fn listing_job(f: &Fixture, ago: i32) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO quality.jobs (tenant_id, kind, parameters, state, worker, lease_until, created_at)
         VALUES ($1::uuid, 'tables', '{\"source\": \"core\"}', 'running', 'w1/0',
                 now() + interval '1 minute', now() - make_interval(secs => $2)) RETURNING id",
    )
    .bind(B)
    .bind(f64::from(ago))
    .fetch_one(&f.db.pool)
    .await
    .unwrap()
}

fn listed(tables: &[(&str, &str)]) -> serde_json::Value {
    let tables: Vec<_> = tables
        .iter()
        .map(|(schema, name)| {
            let columns = [json!({ "name": "id", "data_type": "INTEGER" })];
            json!({ "schema": schema, "name": name, "columns": columns })
        })
        .collect();
    json!({ "source": "core", "tables": tables })
}

async fn kept(f: &Fixture) -> (ListingState, Vec<String>) {
    tenancy::within(tenant(B), async {
        let view = source_tables::view(&f.app, "core").await.unwrap();
        let names = view
            .tables
            .iter()
            .map(|t| format!("{}.{}", t.schema, t.name))
            .collect();
        (view.state, names)
    })
    .await
}

#[tokio::test]
async fn a_listing_gets_its_source_and_its_report_is_kept() {
    let f = fixture().await;
    core(&f).await;
    let job = listing_job(&f, 0).await;
    let (status, body) = f
        .call(
            "GET",
            &format!("/internal/jobs/{B}/{job}"),
            "token-w1",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let details: JobDetails = serde_json::from_slice(&body).unwrap();
    let source = details.source.unwrap();
    assert_eq!((source.name.as_str(), source.table.as_str()), ("core", ""));
    assert_eq!(
        *f.issued.given.lock().unwrap(),
        [(job, "w1".to_string(), "source/core".to_string())]
    );
    assert_eq!(kept(&f).await.0, ListingState::Listing);

    let uri = format!("/internal/jobs/{B}/{job}/report");
    // Another source's, or the system's schemas: refused.
    let mut other = listed(&[("public", "clients")]);
    other["source"] = json!("other");
    for bad in [other, listed(&[("pg_catalog", "pg_class")])] {
        assert_eq!(
            f.call("POST", &uri, "token-w1", Some(bad)).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let report = listed(&[("loans", "credits"), ("public", "clients")]);
    for _ in 0..2 {
        assert_eq!(
            f.call("POST", &uri, "token-w1", Some(report.clone()))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
    }
    assert_eq!(
        kept(&f).await,
        (
            ListingState::Listed,
            vec!["loans.credits".into(), "public.clients".into()]
        )
    );
}

#[tokio::test]
async fn a_late_report_never_replaces_a_newer_listing() {
    let f = fixture().await;
    core(&f).await;
    let older = listing_job(&f, 60).await;
    let newer = listing_job(&f, 0).await;
    for (job, table) in [(newer, "new_one"), (older, "old_one")] {
        let uri = format!("/internal/jobs/{B}/{job}/report");
        let body = listed(&[("public", table)]);
        assert_eq!(
            f.call("POST", &uri, "token-w1", Some(body)).await.0,
            StatusCode::NO_CONTENT
        );
    }
    assert_eq!(kept(&f).await.1, ["public.new_one"]);
}

#[tokio::test]
async fn a_listing_goes_with_its_source() {
    let f = fixture().await;
    core(&f).await;
    let job = listing_job(&f, 0).await;
    let uri = format!("/internal/jobs/{B}/{job}/report");
    let body = listed(&[("public", "clients")]);
    assert_eq!(
        f.call("POST", &uri, "token-w1", Some(body.clone())).await.0,
        StatusCode::NO_CONTENT
    );
    tenancy::within(tenant(B), async {
        assert!(source_store::delete(&f.app, "core").await.unwrap());
    })
    .await;
    // A report that comes after is taken, and kept nowhere.
    assert_eq!(
        f.call("POST", &uri, "token-w1", Some(body)).await.0,
        StatusCode::NO_CONTENT
    );
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM quality.source_tables")
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn a_listing_that_failed_an_attempt_says_it_retries_and_why() {
    let f = fixture().await;
    core(&f).await;
    let job = listing_job(&f, 0).await;
    let fail = |state: &'static str| {
        sqlx::query(
            "UPDATE quality.jobs SET state = $2, attempts = 1, max_attempts = 3,
                    error = 'tables: the source core refused the job' WHERE id = $1",
        )
        .bind(job)
        .bind(state)
    };
    fail("queued").execute(&f.db.pool).await.unwrap();
    let view = tenancy::within(tenant(B), source_tables::view(&f.app, "core"))
        .await
        .unwrap();
    assert_eq!(view.state, ListingState::Retrying);
    assert_eq!((view.attempt, view.max_attempts), (Some(2), Some(3)));
    assert_eq!(view.reason, Some(ListingReason::Connect));

    fail("failed").execute(&f.db.pool).await.unwrap();
    let view = tenancy::within(tenant(B), source_tables::view(&f.app, "core"))
        .await
        .unwrap();
    assert_eq!(view.state, ListingState::Failed);
    assert_eq!(view.reason, Some(ListingReason::Connect));
    assert_eq!(view.attempt, None);
}

#[tokio::test]
async fn a_listing_of_a_sql_server_source_gets_its_credentials_for_duckdb() {
    let f = fixture().await;
    source_on(&f, "sqlserver").await;
    let job = listing_job(&f, 0).await;
    let uri = format!("/internal/jobs/{B}/{job}");
    let (status, body) = f.call("GET", &uri, "token-w1", None).await;
    assert_eq!(status, StatusCode::OK);
    let details: JobDetails = serde_json::from_slice(&body).unwrap();
    let source = details.source.unwrap();
    assert_eq!(
        (source.name.as_str(), source.driver.as_str()),
        ("core", "sqlserver")
    );
    // DuckDB reads it (decision 0077): its credentials, no token for Trino.
    assert!(!source.on_trino() && source.secrets.contains_key("password"));
    assert_eq!(
        *f.issued.given.lock().unwrap(),
        vec![(job, "w1".into(), "source/core".into())]
    );

    let report = format!("{uri}/report");
    let system = listed(&[("sys", "objects")]);
    let refused = f.call("POST", &report, "token-w1", Some(system)).await.0;
    assert_eq!(refused, StatusCode::UNPROCESSABLE_ENTITY);
    let dbo = listed(&[("dbo", "clients")]);
    let kept_now = f.call("POST", &report, "token-w1", Some(dbo)).await.0;
    assert_eq!(kept_now, StatusCode::NO_CONTENT);
    assert_eq!(
        kept(&f).await,
        (ListingState::Listed, vec!["dbo.clients".into()])
    );
}

#[tokio::test]
async fn a_map_of_a_sql_server_source_gets_its_table_and_credentials() {
    let f = fixture().await;
    source_on(&f, "sqlserver").await;
    let job: Uuid = sqlx::query_scalar(
        "INSERT INTO quality.jobs (tenant_id, kind, dataset, parameters, state, worker, lease_until)
         VALUES ($1::uuid, 'map', 'clients', '{\"source\": \"core\", \"table\": \"dbo.clients\"}',
                 'running', 'w1/0', now() + interval '1 minute') RETURNING id",
    )
    .bind(B)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    let (status, body) = f
        .call(
            "GET",
            &format!("/internal/jobs/{B}/{job}"),
            "token-w1",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let details: JobDetails = serde_json::from_slice(&body).unwrap();
    let source = details.source.expect("a map is given its source");
    assert_eq!(
        (source.name.as_str(), source.table.as_str()),
        ("core", "dbo.clients")
    );
    assert!(!source.on_trino() && source.secrets.contains_key("password"));
    assert_eq!(
        *f.issued.given.lock().unwrap(),
        vec![(job, "w1".into(), "source/core".into())]
    );
}
