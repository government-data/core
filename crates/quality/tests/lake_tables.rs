//! The lake listed as a source on the workers' routes (decision 0043),
//! over an ephemeral PostgreSQL: a `lake_tables` job gets credentials for
//! the warehouse's metadata only, recorded first; its report becomes the
//! tenant's listing; one that does not hold together is refused; a late
//! report of an older job never replaces a newer one. Synthetic names only.

pub mod worker;

use axum::http::StatusCode;
use quality::lake_tables;
use quality::source_tables::ListingState;
use quality::worker_api::JobDetails;
use serde_json::json;
use uuid::Uuid;
use worker::*;

/// A running lake listing, queued `ago` seconds ago.
async fn listing_job(f: &Fixture, ago: i32) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO quality.jobs (tenant_id, kind, parameters, state, worker, lease_until, created_at)
         VALUES ($1::uuid, 'lake_tables', '{}', 'running', 'w1/0',
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
            let columns = [json!({ "name": "id", "data_type": "VARCHAR" })];
            json!({ "schema": schema, "name": name, "columns": columns })
        })
        .collect();
    json!({ "tables": tables })
}

async fn kept(f: &Fixture) -> (ListingState, Vec<String>) {
    tenancy::within(tenant(B), async {
        let view = lake_tables::view(&f.app).await.unwrap();
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
async fn a_lake_listing_reads_metadata_only_and_its_report_is_kept() {
    let f = fixture().await;
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
    // The test issuer echoes the policy as the secret.
    let policy: serde_json::Value =
        serde_json::from_str(&details.lake.unwrap().secret_key).unwrap();
    assert!(
        policy.to_string().contains("/*/metadata/*") && !policy.to_string().contains("Put"),
        "the tables' metadata, nothing to write: {policy}"
    );
    let warehouse = "qd-tenant-00000000000000000000000000000001";
    assert_eq!(
        *f.issued.given.lock().unwrap(),
        [(
            job,
            "w1".to_string(),
            format!("warehouse/{warehouse}/metadata")
        )]
    );
    assert_eq!(kept(&f).await.0, ListingState::Listing);

    let uri = format!("/internal/jobs/{B}/{job}/report");
    let unnamed = listed(&[("loans_bronze", "")]);
    assert_eq!(
        f.call("POST", &uri, "token-w1", Some(unnamed)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let report = listed(&[("cards_bronze", "cards"), ("loans_bronze", "loans")]);
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
            vec!["cards_bronze.cards".into(), "loans_bronze.loans".into()]
        )
    );
}

#[tokio::test]
async fn a_late_lake_report_never_replaces_a_newer_listing() {
    let f = fixture().await;
    let older = listing_job(&f, 60).await;
    let newer = listing_job(&f, 0).await;
    for (job, table) in [(newer, "new_one"), (older, "old_one")] {
        let uri = format!("/internal/jobs/{B}/{job}/report");
        assert_eq!(
            f.call(
                "POST",
                &uri,
                "token-w1",
                Some(listed(&[("x_bronze", table)]))
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
    }
    assert_eq!(kept(&f).await.1, ["x_bronze.new_one"]);
}
