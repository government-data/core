//! The workers' routes on an ephemeral PostgreSQL, connected as the
//! application's role with row security per tenant: a job is served only
//! to the machine whose worker holds it, under the job's tenant, and its
//! report lands in that tenant's results, with each field's profile,
//! replacing a repeated one; an extraction's report publishes its cut and
//! queues its evaluation. An evaluation gets its tenant's entity key, and
//! its ICD lands with its results, each entity by a hash only.

pub mod worker;

use axum::http::StatusCode;
use quality::manifest::{EvaluateManifest, ExtractManifest};
use quality::worker_api::JobDetails;
use serde_json::json;
use uuid::Uuid;
use worker::*;

#[tokio::test]
async fn a_job_is_served_only_to_the_machine_holding_it_under_its_tenant() {
    let f = fixture().await;
    let job = f.job(A, "w1/0", 60).await;
    let (status, body) = f
        .call(
            "GET",
            &format!("/internal/jobs/{A}/{job}"),
            "token-w1",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let details: JobDetails = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        (details.kind.as_str(), details.dataset.as_deref()),
        ("evaluate", Some("clientes"))
    );
    // Credentials of its own, read-only on its tenant's warehouse.
    let lake = details.lake.unwrap();
    assert_eq!(lake.warehouse, "qd-tenant-00000000000000000000000000000001");
    assert_eq!(lake.access_key, format!("job-{job}"));
    assert_eq!(
        lake.secret_key,
        quality::lake::read_only_policy(&lake.warehouse)
    );
    // The catalog's snapshot, read as the job's tenant.
    assert_eq!(details.catalog.unwrap().commit, format!("commit-of-{A}"));

    // Another machine, a prefix of its name, another tenant, no token.
    let other = "00000000-0000-0000-0000-00000000000b";
    for (uri, token) in [
        (format!("/internal/jobs/{A}/{job}"), "token-w2"),
        (format!("/internal/jobs/{A}/{job}"), "token-w"),
        (format!("/internal/jobs/{other}/{job}"), "token-w1"),
    ] {
        assert_eq!(
            f.call("GET", &uri, token, None).await.0,
            StatusCode::NOT_FOUND,
            "{uri} {token}"
        );
    }
    assert_eq!(
        f.call("GET", &format!("/internal/jobs/{A}/{job}"), "nobody", None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    // A lease that ran out: no longer its.
    let expired = f.job(A, "w1/0", -1).await;
    assert_eq!(
        f.call(
            "GET",
            &format!("/internal/jobs/{A}/{expired}"),
            "token-w1",
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

/// The routes without the tenant (decision 0076) serve the installation's
/// jobs, the main tenant's, and only those.
#[tokio::test]
async fn the_route_without_the_tenant_serves_the_main_tenants_job() {
    let f = fixture().await;
    let main = "00000000-0000-0000-0000-000000000001";
    let job = f.job(main, "w1/0", 60).await;
    let (status, body) = f
        .call("GET", &format!("/internal/jobs/{job}"), "token-w1", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let details: JobDetails = serde_json::from_slice(&body).unwrap();
    assert_eq!(details.catalog.unwrap().commit, format!("commit-of-{main}"));
    let other = f
        .job("00000000-0000-0000-0000-00000000000b", "w1/0", 60)
        .await;
    assert_eq!(
        f.call("GET", &format!("/internal/jobs/{other}"), "token-w1", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

/// A role's client is shared by its machines (decision 0066): a token that
/// names its machine is served only that machine's jobs, its report too.
#[tokio::test]
async fn a_roles_machine_is_served_only_its_own_jobs() {
    let f = fixture().await;
    let job = f.job(A, "qd-worker/worker-01/0", 60).await;
    let uri = format!("/internal/jobs/{A}/{job}");
    assert_eq!(
        f.call("GET", &uri, "token-qd-worker@worker-01", None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        *f.issued.given.lock().unwrap().last().unwrap(),
        (
            job,
            "qd-worker/worker-01".to_string(),
            "entity_key".to_string()
        ),
        "recorded to the machine"
    );
    // Another machine of the role, one whose name it starts with, and the
    // role's own name as a machine: neither reads nor reports it.
    for token in [
        "token-qd-worker@worker-02",
        "token-qd-worker@worker-0",
        "token-qd-worker@worker",
    ] {
        assert_eq!(
            f.call("GET", &uri, token, None).await.0,
            StatusCode::NOT_FOUND,
            "{token}"
        );
        assert_eq!(
            f.call("POST", &format!("{uri}/report"), token, Some(report(1)))
                .await
                .0,
            StatusCode::NOT_FOUND,
            "{token}"
        );
    }
    // A machine's token is not a client's of the same name with a secret:
    // that one's workers are `qd-worker/<slot>`, not a machine's.
    let own = f.job(A, "qd-worker/0", 60).await;
    let own_uri = format!("/internal/jobs/{A}/{own}");
    assert_eq!(
        f.call("GET", &own_uri, "token-qd-worker@worker-01", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    // A token without the claim is as before: its client's workers.
    assert_eq!(
        f.call("GET", &own_uri, "token-qd-worker", None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        f.call(
            "POST",
            &format!("{uri}/report"),
            "token-qd-worker@worker-01",
            Some(report(1))
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn a_rules_counts_keep_what_the_charts_group_them_by() {
    let f = fixture().await;
    let job = f.job(B, "w1/3", 60).await;
    let uri = format!("/internal/jobs/{B}/{job}/report");
    assert_eq!(
        f.call("POST", &uri, "token-w1", Some(report(7))).await.0,
        StatusCode::NO_CONTENT
    );
    let some = |s: &str| Some(s.to_string());
    assert_eq!(
        f.grouping(B).await,
        [
            (
                "V-01".to_string(),
                some("Nit"),
                some("required"),
                some("completeness"),
                some("blocking")
            ),
            // An older worker's report: counts only, the rest empty.
            ("V-02".to_string(), None, None, None, None),
        ]
    );
}

#[tokio::test]
async fn a_report_lands_in_the_jobs_tenant_and_replaces_a_repeated_one() {
    let f = fixture().await;
    let job = f.job(B, "w1/3", 60).await;
    let uri = format!("/internal/jobs/{B}/{job}/report");
    assert_eq!(
        f.call("POST", &uri, "token-w1", Some(report(7))).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.call("POST", &uri, "token-w1", Some(report(5))).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.failures(B).await,
        [("V-01".to_string(), 5), ("V-02".to_string(), 0)]
    );
    // Each field's profile, once though reported twice; an older worker's
    // report, without them, leaves none.
    let profiles = [
        (
            "Nit".to_string(),
            3,
            97,
            Some("999999999".to_string()),
            90,
            None,
        ),
        (
            "Updated".to_string(),
            1,
            40,
            Some("9999-99-99".to_string()),
            99,
            Some(vec![0, 50, 20, 10, 10, 5, 4]),
        ),
    ];
    assert_eq!(f.field_quality(B).await, profiles);
    let mut older = report(5);
    older.as_object_mut().unwrap().remove("fields");
    assert_eq!(
        f.call("POST", &uri, "token-w1", Some(older)).await.0,
        StatusCode::NO_CONTENT
    );
    assert!(f.field_quality(B).await.is_empty());

    // Counts the tables refuse, a bad date, a rule twice: the worker's error.
    let mut bad = report(101);
    assert_eq!(
        f.call("POST", &uri, "token-w1", Some(bad.clone())).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    bad = report(1);
    bad["cut"] = json!("2021-02-30");
    assert_eq!(
        f.call("POST", &uri, "token-w1", Some(bad)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut twice = report(1);
    twice["rules"][1]["rule"] = json!("V-01");
    assert_eq!(
        f.call("POST", &uri, "token-w1", Some(twice)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        f.failures(B).await.len(),
        2,
        "a refused report changes nothing"
    );
}

#[tokio::test]
async fn an_evaluation_leaves_its_manifest_once_and_its_results_point_to_it() {
    let f = fixture().await;
    let job = f.job(B, "w1/0", 60).await;
    let uri = format!("/internal/jobs/{B}/{job}/report");
    for _ in 0..2 {
        assert_eq!(
            f.call("POST", &uri, "token-w1", Some(report(3))).await.0,
            StatusCode::NO_CONTENT
        );
    }
    let objects = f.bucket.0.lock().unwrap().clone();
    let key = format!("manifests/clientes/2021-03-10/evaluate/{job}.json");
    assert_eq!(objects.len(), 1, "a repeated report writes nothing more");
    let (warehouse, written, body) = &objects[0];
    assert_eq!(
        (warehouse.as_str(), written.as_str()),
        ("qd-tenant-00000000000000000000000000000001", key.as_str())
    );
    let manifest: EvaluateManifest = serde_json::from_slice(body).unwrap();
    assert_eq!(
        (
            manifest.job,
            manifest.catalog_commit.as_str(),
            manifest.rules[0].failures
        ),
        (job, "abc123", 3)
    );
    let t = tenancy::Tenant {
        id: B.into(),
        slug: String::new(),
    };
    let stored: Option<String> = tenancy::within(t, async {
        sqlx::query_scalar("SELECT manifest FROM quality.run_summary")
            .fetch_one(&f.app)
            .await
            .unwrap()
    })
    .await;
    assert_eq!(stored, Some(key));
}

#[tokio::test]
async fn an_extraction_publishes_its_cut_and_queues_its_evaluation_once() {
    let f = fixture().await;
    let extract = f.cut(B).await;
    let uri = format!("/internal/jobs/{B}/{extract}/report");
    // Impossible counts, another date: refused, and nothing published.
    let mut wrong = extracted(-1);
    assert_eq!(
        f.call("POST", &uri, "token-w1", Some(wrong)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    wrong = extracted(10);
    wrong["cut"] = json!("2021-03-11");
    assert_eq!(
        f.call("POST", &uri, "token-w1", Some(wrong)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(f.cut_state().await.0, "extracting");

    // With the silver snapshot it wrote (decision 0061).
    let mut written = extracted(10);
    written["silver_snapshot"] = json!("7135000000000000002");
    for _ in 0..2 {
        assert_eq!(
            f.call("POST", &uri, "token-w1", Some(written.clone()))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
    }
    let (state, snapshot, evaluate) = f.cut_state().await;
    assert_eq!(
        (state.as_str(), snapshot.as_deref()),
        ("evaluating", Some("7135000000000000001"))
    );
    let queued: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
        "SELECT id, parameters FROM quality.jobs WHERE kind = 'evaluate' AND tenant_id = $1::uuid",
    )
    .bind(B)
    .fetch_all(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(queued.len(), 1, "a repeated report queues nothing more");
    assert_eq!(Some(queued[0].0), evaluate);
    assert_eq!(queued[0].1["snapshot"], "7135000000000000001");
    assert_eq!(queued[0].1["cut"], "2021-03-10");
    assert_eq!(queued[0].1["silver_snapshot"], "7135000000000000002");
    // The cut's replacement number beside its id, for the logs (0026).
    assert_eq!(queued[0].1["run"], 1);

    let objects = f.bucket.0.lock().unwrap().clone();
    assert_eq!(objects.len(), 1);
    assert_eq!(
        objects[0].1,
        format!("manifests/clientes/2021-03-10/extract/{extract}.json")
    );
    let manifest: ExtractManifest = serde_json::from_slice(&objects[0].2).unwrap();
    assert_eq!((manifest.ranges[0].rows, manifest.schema.len()), (10, 1));
    assert_eq!(
        manifest.silver_snapshot.as_deref(),
        Some("7135000000000000002")
    );

    // Its evaluation's report ends the chain.
    let evaluate = evaluate.unwrap();
    f.hold(evaluate).await;
    assert_eq!(
        f.call(
            "POST",
            &format!("/internal/jobs/{B}/{evaluate}/report"),
            "token-w1",
            Some(report(2))
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(f.cut_state().await.0, "done");
}

#[tokio::test]
async fn an_evaluation_gets_its_tenants_entity_key_sealed_where_it_is_kept() {
    let f = fixture().await;
    let mut keys = Vec::new();
    for tenant in [A, A] {
        let job = f.job(tenant, "w1/0", 60).await;
        let uri = format!("/internal/jobs/{tenant}/{job}");
        let (status, body) = f.call("GET", &uri, "token-w1", None).await;
        assert_eq!(status, StatusCode::OK);
        let details: JobDetails = serde_json::from_slice(&body).unwrap();
        let key = details.entity_key.clone().unwrap().0;
        assert!(!format!("{details:?}").contains(&key), "never printed");
        assert!(f.issued.given.lock().unwrap().contains(&(
            job,
            "w1".to_string(),
            "entity_key".to_string()
        )));
        keys.push(key);
    }
    // The same every time.
    assert_eq!(keys[0], keys[1]);
    assert_eq!(keys[0].len(), 64);
    let sealed: String =
        sqlx::query_scalar("SELECT sealed FROM quality.entity_keys WHERE tenant_id = $1::uuid")
            .bind(A)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert!(sealed.starts_with("sealed:") && !sealed.contains(&keys[0]));
}

/// A summary's IGCD, entities and bands.
type Summary = (
    Option<f64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);
/// An entity's hash, ICD, band and segment.
type EntityRow = (String, Option<f64>, Option<String>, Option<String>);
/// A distribution check as stored: variable, dimension, group, the 10th
/// percentile, amounts and high outliers.
type Check = (
    String,
    String,
    String,
    Option<f64>,
    Option<i64>,
    Option<i64>,
);

#[tokio::test]
async fn a_cuts_icd_lands_with_its_results_each_entity_by_a_hash_only() {
    let f = fixture().await;
    let job = f.job(B, "w1/3", 60).await;
    let uri = format!("/internal/jobs/{B}/{job}/report");
    for _ in 0..2 {
        let (status, _) = f.call("POST", &uri, "token-w1", Some(report(5))).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    let summary: Summary = sqlx::query_as(
        "SELECT igcd, entities, band_high, band_medium, band_low
               FROM quality.run_summary WHERE tenant_id = $1::uuid",
    )
    .bind(B)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(summary, (Some(0.825), Some(2), Some(1), Some(0), Some(1)));
    let rows: Vec<EntityRow> = sqlx::query_as(
        "SELECT entity, icd, band, segment FROM quality.entity_icd
          WHERE tenant_id = $1::uuid ORDER BY entity",
    )
    .bind(B)
    .fetch_all(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "once though reported twice");
    assert_eq!(
        (rows[0].2.as_deref(), rows[0].3.as_deref()),
        (Some("high"), Some("PN"))
    );
    assert_eq!(rows[1].2.as_deref(), Some("low"));
    let mean: Option<f64> = sqlx::query_scalar(
        "SELECT mean_icd FROM quality.field_quality WHERE tenant_id = $1::uuid AND field = 'Nit'",
    )
    .bind(B)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(mean, Some(0.8));
    // Each office once, the empty one as ''.
    let groups: Vec<(String, String, i64, i64, Option<f64>)> = sqlx::query_as(
        "SELECT dimension, grp, records, serious, mean_icd FROM quality.group_metrics
          WHERE tenant_id = $1::uuid ORDER BY grp",
    )
    .bind(B)
    .fetch_all(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(
        groups,
        [
            ("Office".to_string(), String::new(), 40, 10, None),
            ("Office".to_string(), "O-1".to_string(), 60, 6, Some(0.9)),
        ]
    );
    // The drift variable's percentiles, and the outliers by office.
    let checks: Vec<Check> = sqlx::query_as(
        "SELECT variable, dimension, grp, percentiles[11], records, high
           FROM quality.distribution_checks WHERE tenant_id = $1::uuid ORDER BY dimension",
    )
    .bind(B)
    .fetch_all(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(
        checks,
        [
            (
                "Income".into(),
                String::new(),
                String::new(),
                Some(10.0),
                None,
                None
            ),
            (
                "Income".into(),
                "Office".into(),
                "O-1".into(),
                None,
                Some(60),
                Some(2)
            ),
        ]
    );
    // Each stage's time, once, in the order it ran.
    let stages: Vec<(String, i32, i64)> = sqlx::query_as(
        "SELECT stage, position, ms FROM quality.job_stats
          WHERE tenant_id = $1::uuid ORDER BY position",
    )
    .bind(B)
    .fetch_all(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(
        stages,
        [("load".to_string(), 1, 700), ("rules".to_string(), 2, 500)]
    );

    // An entity that is not a hash (a document sent by mistake) is refused.
    let mut keyed = report(5);
    keyed["entities"][0][0] = json!("12345678");
    let (status, _) = f.call("POST", &uri, "token-w1", Some(keyed)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn a_cuts_sfc_cells_land_with_what_the_guide_derives() {
    let f = fixture().await;
    let job = f.job(B, "w1/3", 60).await;
    let uri = format!("/internal/jobs/{B}/{job}/report");
    for _ in 0..2 {
        let (status, _) = f.call("POST", &uri, "token-w1", Some(report(5))).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    // ceil(0.90 · 100) = 90: Phone passes by 2, City misses 5. Once though
    // reported twice.
    let cells: Vec<(String, i64, i64, bool, i64, i64)> = sqlx::query_as(
        "SELECT cell, ok, total, passes, missing, margin FROM quality.cells
          WHERE tenant_id = $1::uuid ORDER BY cell",
    )
    .bind(B)
    .fetch_all(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(
        cells,
        [
            ("City".to_string(), 85, 100, false, 5, 0),
            ("Phone".to_string(), 92, 100, true, 0, 2),
        ]
    );
}

/// A queue as stored: dimension, group, since, open, resolved, priority.
type Queue = (String, String, String, i64, i64, f64);

#[tokio::test]
async fn a_cuts_queues_land_with_its_results_by_the_cut_they_opened_at() {
    let f = fixture().await;
    let job = f.job(B, "w1/3", 60).await;
    let uri = format!("/internal/jobs/{B}/{job}/report");
    for _ in 0..2 {
        let (status, _) = f.call("POST", &uri, "token-w1", Some(report(5))).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    let queues: Vec<Queue> = sqlx::query_as(
        "SELECT dimension, grp, since::text, open, resolved, priority FROM quality.queue_findings
          WHERE tenant_id = $1::uuid ORDER BY since",
    )
    .bind(B)
    .fetch_all(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(
        queues,
        [
            (
                "Office".into(),
                "O-1".into(),
                "2021-02-10".into(),
                4,
                1,
                1.2
            ),
            (
                "Office".into(),
                String::new(),
                "2021-03-10".into(),
                2,
                0,
                0.4
            ),
        ],
        "once though reported twice, the empty group as ''"
    );

    // A queue opened after its cut is refused, and nothing is kept.
    let job = f.job(B, "w1/3", 60).await;
    let uri = format!("/internal/jobs/{B}/{job}/report");
    let mut late = report(5);
    late["queues"][0]["since"] = json!("2021-04-10");
    let (status, _) = f.call("POST", &uri, "token-w1", Some(late)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
