//! What a job is given to read and write (decisions 0030, 0036 to 0038,
//! 0053), on the workers' routes over an ephemeral PostgreSQL: an
//! extraction gets its source and its bronze table's credentials, a map the
//! source it names and its report publishes the dataset and opens its first
//! cut, a maintenance its dataset's tables and its report a manifest; every
//! credential given is recorded.

pub mod worker;

use axum::http::StatusCode;
use quality::lake::LakeAccess;
use quality::proposal::{ColumnProfile, Looks, TableProfile};
use quality::source::Source;
use quality::source_store;
use quality::worker_api::JobDetails;
use serde_json::json;
use uuid::Uuid;
use worker::*;

#[tokio::test]
async fn an_extraction_gets_its_source_and_every_credential_given_is_recorded() {
    let f = fixture().await;
    async {
        let source = Source {
            name: "core".into(),
            driver: "postgres".into(),
            fields: [("host", "db.example"), ("database", "core")]
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .into(),
            secrets: [("user", "reader"), ("password", "synthetic")]
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .into(),
        };
        source_store::put(&f.app, &Mirror, &source).await.unwrap();
    }
    .await;
    let extract = f.cut().await;
    let (status, body) = f
        .call(
            "GET",
            &format!("/internal/jobs/{B}/{extract}"),
            "token-w1",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let details: JobDetails = serde_json::from_slice(&body).unwrap();
    let source = details.source.unwrap();
    assert_eq!(
        (source.name.as_str(), source.table.as_str()),
        ("core", "public.clients")
    );
    assert_eq!(source.secrets["password"], "synthetic");
    assert!(source.previous.is_none());
    assert!(
        details.lake.is_some(),
        "its bronze and silver tables' (0037, 0061)"
    );

    // An evaluation's warehouse credentials are recorded the same way.
    let evaluate = f.job(B, "w1/0", 60).await;
    let uri = format!("/internal/jobs/{B}/{evaluate}");
    assert_eq!(
        f.call("GET", &uri, "token-w1", None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        *f.issued.given.lock().unwrap(),
        [
            (extract, "w1".to_string(), "source/core".to_string()),
            (
                extract,
                "w1".to_string(),
                "warehouse/qd-tenant-00000000000000000000000000000001/clientes_bronze.clientes"
                    .to_string()
            ),
            (
                extract,
                "w1".to_string(),
                "warehouse/qd-tenant-00000000000000000000000000000001/clientes_silver.clientes"
                    .to_string()
            ),
            (
                evaluate,
                "w1".to_string(),
                "warehouse/qd-tenant-00000000000000000000000000000001".to_string()
            ),
            // And its key for its entities' hashes (0057).
            (evaluate, "w1".to_string(), "entity_key".to_string()),
        ]
    );

    // A credential that cannot be recorded is not given.
    f.issued
        .down
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        f.call("GET", &uri, "token-w1", None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

/// The grant a job was given (the Issuer's echo).
fn grant(access: &LakeAccess) -> quality::lake::LakeGrant {
    serde_json::from_str(&access.secret_key).unwrap()
}

/// Where a job's grant creates tables, and the tables whose files it
/// writes, by namespace, name and location.
fn writes(access: &LakeAccess) -> (Vec<String>, Vec<(String, String, String)>) {
    let grant = grant(access);
    let tables = grant.write_tables.into_iter();
    let tables = tables.map(|t| (t.namespace, t.name, t.location)).collect();
    (grant.create_tables_in, tables)
}

fn placed(namespace: &str, location: &str) -> (String, String, String) {
    (namespace.into(), "clientes".into(), location.into())
}

#[tokio::test]
async fn an_extraction_creates_its_table_then_asks_again_for_its_files() {
    let f = fixture().await;
    let extract = f.cut().await;
    let w = "qd-tenant-00000000000000000000000000000001";
    let (status, body) = f
        .call(
            "GET",
            &format!("/internal/jobs/{B}/{extract}"),
            "token-w1",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let details: JobDetails = serde_json::from_slice(&body).unwrap();
    // Its namespaces are made by the application, not by the job.
    assert_eq!(
        *f.tables.1.lock().unwrap(),
        ["clientes_bronze", "clientes_silver"]
    );
    // No table yet: it may create one in each namespace, and write no file.
    let namespaces = vec!["clientes_bronze".to_string(), "clientes_silver".into()];
    let access = details.lake.unwrap();
    assert_eq!(grant(&access).warehouse, w);
    assert_eq!(writes(&access), (namespaces.clone(), vec![]));

    // Created: asking again gives it those tables' files, as the catalog says.
    for (table, prefix) in [
        ("clientes_bronze.clientes", "tables/u-1"),
        ("clientes_silver.clientes", "tables/u-2"),
    ] {
        f.tables
            .0
            .lock()
            .unwrap()
            .push((table.into(), prefix.into()));
    }
    let (status, body) = f
        .call(
            "GET",
            &format!("/internal/jobs/{B}/{extract}/lake"),
            "token-w1",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let again: LakeAccess = serde_json::from_slice(&body).unwrap();
    let tables = vec![
        placed("clientes_bronze", "tables/u-1"),
        placed("clientes_silver", "tables/u-2"),
    ];
    assert_eq!(writes(&again), (namespaces, tables));
    let resource = format!("warehouse/{w}/clientes_bronze.clientes");
    let given = f.issued.given.lock().unwrap().clone();
    assert_eq!(
        given
            .iter()
            .filter(|(j, _, r)| *j == extract && *r == resource)
            .count(),
        2,
        "each set recorded"
    );

    // Not an extraction, or another machine: nothing.
    let evaluate = f.job(B, "w1/0", 60).await;
    for (job, token) in [(evaluate, "token-w1"), (extract, "token-w2")] {
        assert_eq!(
            f.call(
                "GET",
                &format!("/internal/jobs/{B}/{job}/lake"),
                token,
                None
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn a_map_gets_the_source_it_names_and_its_report_publishes_the_dataset_and_its_first_cut() {
    let f = fixture().await;
    async {
        let source = Source {
            name: "core".into(),
            driver: "postgres".into(),
            fields: [("host", "db.example"), ("database", "core")]
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .into(),
            secrets: [("user", "reader"), ("password", "synthetic")]
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .into(),
        };
        source_store::put(&f.app, &Mirror, &source).await.unwrap();
    }
    .await;
    let job: Uuid = sqlx::query_scalar(
        "INSERT INTO quality.jobs (tenant_id, kind, dataset, parameters, state, worker, lease_until)
         VALUES ($1::uuid, 'map', 'loans', '{\"source\": \"core\", \"table\": \"public.loans\", \"sample\": 10000, \"requested_by\": \"00000000-0000-0000-0000-0000000000aa\"}',
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
    let source = details.source.unwrap();
    assert_eq!(
        (source.name.as_str(), source.table.as_str()),
        ("core", "public.loans")
    );

    let report = |table: &str| {
        json!({ "profile": TableProfile {
            source: "core".into(),
            table: table.into(),
            row_estimate: None,
            sampled: 100,
            columns: vec![ColumnProfile {
                name: "loan_id".into(),
                declared_type: "INTEGER".into(),
                nullable: false,
                primary_key: true,
                empty: 0,
                distinct: 100,
                min_length: Some(1),
                max_length: Some(3),
                looks: Looks::default(),
            }],
        } })
    };
    let uri = format!("/internal/jobs/{B}/{job}/report");
    // Another table than the one asked for: refused, nothing published.
    assert_eq!(
        f.call("POST", &uri, "token-w1", Some(report("public.other")))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    for _ in 0..2 {
        assert_eq!(
            f.call("POST", &uri, "token-w1", Some(report("public.loans")))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
    }
    let published = f.drafted.0.lock().unwrap().clone();
    assert_eq!(
        published.len(),
        1,
        "a repeated report finds the dataset there"
    );
    assert_eq!(published[0].0, "loans");
    assert!(published[0]
        .1
        .contains("from: { source: core, table: \"public.loans\" }"));
    assert!(
        published[0].1.contains("key: [loan_id]"),
        "a field named as its column"
    );
    // Its first cut, opened once, by whoever asked, with its extraction.
    let cuts = cuts_of(&f.db.pool, "loans").await;
    assert_eq!(cuts.len(), 1, "a repeated report finds its cut opened");
    assert_eq!(
        cuts[0],
        (
            "extracting".to_string(),
            Some("00000000-0000-0000-0000-0000000000aa".to_string()),
            true
        )
    );
}

/// The cuts of `dataset` in tenant B: state, who asked, whether extracting.
async fn cuts_of(pool: &sqlx::PgPool, dataset: &str) -> Vec<(String, Option<String>, bool)> {
    sqlx::query_as(
        "SELECT state, requested_by::text, extract_job IS NOT NULL FROM quality.cuts
          WHERE tenant_id = $1::uuid AND dataset = $2",
    )
    .bind(B)
    .bind(dataset)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_maintenance_gets_its_datasets_tables_and_leaves_its_manifest() {
    let f = fixture().await;
    // Its bronze and silver tables (0061), and another dataset's.
    for (table, prefix) in [
        ("clientes_bronze.clientes", "tables/u-1"),
        ("clientes_silver.clientes", "tables/u-2"),
        ("loans_silver.loans", "tables/u-3"),
    ] {
        f.tables
            .0
            .lock()
            .unwrap()
            .push((table.into(), prefix.into()));
    }
    let job: Uuid = sqlx::query_scalar(
        "INSERT INTO quality.jobs (tenant_id, kind, dataset, state, worker, lease_until)
         VALUES ($1::uuid, 'maintain', 'clientes', 'running', 'w1/0', now() + interval '1 minute')
         RETURNING id",
    )
    .bind(B)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    let w = "qd-tenant-00000000000000000000000000000001";
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
    // Its tables' files where the catalog says they are; it creates nothing.
    let tables = vec![
        placed("clientes_bronze", "tables/u-1"),
        placed("clientes_silver", "tables/u-2"),
    ];
    assert_eq!(writes(&details.lake.unwrap()), (vec![], tables));
    for namespace in ["clientes_bronze", "clientes_silver"] {
        assert!(f.issued.given.lock().unwrap().contains(&(
            job,
            "w1".into(),
            format!("warehouse/{w}/{namespace}")
        )));
    }

    let report = |files_after: i64, orphan_days: i64| {
        json!({ "snapshot_days": 396, "orphan_days": orphan_days, "tables": [{
            "table": "clientes", "files_before": 20, "files_after": files_after,
            "snapshots_expired": 19, "orphans_deleted": 20
        }] })
    };
    let uri = format!("/internal/jobs/{B}/{job}/report");
    for bad in [report(-1, 7), report(1, 3)] {
        assert_eq!(
            f.call("POST", &uri, "token-w1", Some(bad)).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        f.call("POST", &uri, "token-w1", Some(report(1, 7))).await.0,
        StatusCode::NO_CONTENT
    );
    let objects = f.bucket.0.lock().unwrap().clone();
    assert_eq!(objects.len(), 1);
    assert_eq!(
        objects[0].1,
        format!("manifests/clientes/maintain/{job}.json")
    );
}
