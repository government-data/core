//! Settings → Lake's admin keys and the replication (0066 step 11), end to
//! end: the real routes and use cases on PostgreSQL, each lake's keys kept
//! sealed and never answered, the lakes asked at a stand-in of their gate.
//! Needs Docker.

mod machines_world;

use std::time::{Duration, SystemTime};

use auth_gateway::MachineRole;
use axum::http::StatusCode;
use infrastructure::PgMachines;
use machines_world::*;
use serde_json::{json, Value};

#[tokio::test]
async fn the_replicas_admin_keys_are_kept_sealed_never_answered_and_audited() {
    let w = world(false, 100).await;
    let admin = sign_in(&w.app, ADMIN).await;
    let user = sign_in(&w.app, USER).await;
    let (get, put) = (
        ("GET", "/lake/replica/admin-keys"),
        ("PUT", "/lake/replica/admin-keys"),
    );
    let none = json!({ "set": false, "access_key": null, "set_at": null });
    assert_eq!(
        call(&w.app, get, Some((&admin, false)), None).await,
        (StatusCode::OK, none)
    );
    let secret = "5e".repeat(32);
    let keys =
        |access: &str, secret: &str| Some(json!({ "access_key": access, "secret_key": secret }));
    let pasted = keys("qd-platform-admin", &secret);
    let (status, _) = call(&w.app, get, Some((&user, true)), None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "without admin:ManageMachines"
    );
    let (status, _) = call(&w.app, put, Some((&user, true)), pasted.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = call(&w.app, put, Some((&admin, false)), pasted.clone()).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("csrf_mismatch"))
    );
    let (status, _) = call(&w.app, put, None, pasted.clone()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    for (bad, field) in [
        (keys("qd platform", &secret), "access_key"),
        (keys("qd-platform-admin", "not-the-secret"), "secret_key"),
    ] {
        let (status, body) = call(&w.app, put, Some((&admin, true)), bad).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body["fields"][field].is_string(), "{body}");
    }

    // Kept, answered without the secret, the same when read.
    for _ in 0..2 {
        let (status, body) = call(&w.app, put, Some((&admin, true)), pasted.clone()).await;
        assert_eq!(
            (status, body["set"].as_bool()),
            (StatusCode::OK, Some(true))
        );
        assert_eq!(body["access_key"], "qd-platform-admin");
        assert!(off(body["set_at"].as_i64().unwrap(), 0) < 60);
        assert!(!body.to_string().contains(&secret));
        assert_eq!(
            call(&w.app, get, Some((&admin, false)), None).await,
            (status, body)
        );
    }
    // One row, its secret sealed; audited twice, by the person, never with it.
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT lake, access_key, secret_key FROM lake_admin_keys")
            .fetch_all(&w.pool)
            .await
            .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].0.as_str(), rows[0].1.as_str()),
        ("replica", "qd-platform-admin")
    );
    assert_ne!(rows[0].2, secret, "sealed");
    assert_eq!(sealer().decrypt(&rows[0].2).unwrap(), secret);
    let events: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT event_type, action, resource, admin_id::text FROM admin_audit_events \
         WHERE resource = 'lake/replica'",
    )
    .fetch_all(&w.pool)
    .await
    .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        (events[0].0.as_str(), events[0].1.as_str()),
        ("secrets_changed", "lake:admin-keys")
    );
    assert!(events[0].3.is_some(), "by the person");
    let audited: Vec<(String,)> =
        sqlx::query_as("SELECT row_to_json(e)::text FROM admin_audit_events e")
            .fetch_all(&w.pool)
            .await
            .unwrap();
    assert!(audited.iter().all(|(row,)| !row.contains(&secret)));
}

#[tokio::test]
async fn the_replica_is_asked_by_its_machines_name_with_the_keys_pasted() {
    let w = world(false, 100).await;
    let admin = sign_in(&w.app, ADMIN).await;
    let user = sign_in(&w.app, USER).await;
    let state = ("GET", "/lake/replica/state");
    let (status, _) = call(&w.app, state, Some((&user, true)), None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "without admin:ManageMachines"
    );
    let (status, body) = call(&w.app, state, Some((&admin, false)), None).await;
    assert_eq!(
        (status, body["state"].as_str()),
        (StatusCode::OK, Some("no_machine"))
    );
    // The replica's machine, approved by the installer's way for the test.
    let pg = PgMachines::new(w.pool.clone());
    let made = auth_gateway::MachineRegistry::create_approved(
        &pg,
        "localhost",
        MachineRole::LakeReplica,
        SystemTime::now(),
    );
    made.await.unwrap().unwrap();
    let (_, body) = call(&w.app, state, Some((&admin, false)), None).await;
    assert_eq!(
        (body["state"].as_str(), body["machine"].as_str()),
        (Some("no_keys"), Some("localhost"))
    );
    let secret = "7c".repeat(32);
    let pasted = json!({ "access_key": "qd-platform-admin", "secret_key": secret });
    let put = ("PUT", "/lake/replica/admin-keys");
    let (status, _) = call(&w.app, put, Some((&admin, true)), Some(pasted)).await;
    assert_eq!(status, StatusCode::OK);
    let info = r#"{"info":{"backend":{"backendType":"Erasure","onlineDisks":2,"offlineDisks":1}}}"#;
    let seen = answer_once(&w.gate, info);
    let (status, body) = call(&w.app, state, Some((&admin, false)), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (
            body["state"].as_str(),
            body["drives_online"].as_u64(),
            body["drives_offline"].as_u64()
        ),
        (Some("answered"), Some(2), Some(1))
    );
    assert!(!body.to_string().contains(&secret));
    // Signed with the secret opened from its sealed row, which it does not carry.
    let head = seen.join().unwrap();
    assert!(!head.contains(&secret));
    let header = |name: &str| {
        let line = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with(name))
            .unwrap();
        line.split_once(": ").unwrap().1.to_string()
    };
    let date = chrono::NaiveDateTime::parse_from_str(&header("x-amz-date"), "%Y%m%dT%H%M%SZ");
    let port = w.gate.local_addr().unwrap().port();
    let host = format!("localhost:{port}");
    let hash = sigv4::payload_hash(b"");
    let signed = sigv4::sign(
        "s3",
        "qd-platform-admin",
        &secret,
        "GET",
        "/rustfs/admin/v3/info",
        &[
            ("content-length", "0"),
            ("host", &host),
            ("x-amz-content-sha256", &hash),
        ],
        date.unwrap().and_utc(),
    );
    assert_eq!(header("authorization"), signed.authorization);
}

#[tokio::test]
async fn the_main_lakes_keys_are_kept_as_pool_main_and_it_is_asked_at_its_endpoint() {
    let w = world(false, 100).await;
    let admin = sign_in(&w.app, ADMIN).await;
    let user = sign_in(&w.app, USER).await;
    let (keys, state) = (
        ("GET", "/lake/main/admin-keys"),
        ("GET", "/lake/main/state"),
    );
    for route in [keys, state, ("PUT", "/lake/main/admin-keys")] {
        let (status, _) = call(&w.app, route, Some((&user, true)), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{route:?}");
    }
    let (_, body) = call(&w.app, keys, Some((&admin, false)), None).await;
    assert_eq!(body["set"], json!(false));
    let port = w.gate.local_addr().unwrap().port();
    let endpoint = format!("http://localhost:{port}");
    let (_, body) = call(&w.app, state, Some((&admin, false)), None).await;
    assert_eq!(
        (body["state"].as_str(), body["endpoint"].as_str()),
        (Some("no_keys"), Some(endpoint.as_str()))
    );
    let secret = "5e".repeat(32);
    let pasted = json!({ "access_key": "qd-platform-admin", "secret_key": secret });
    let put = ("PUT", "/lake/main/admin-keys");
    let (status, body) = call(&w.app, put, Some((&admin, false)), Some(pasted.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "without CSRF: {body}");
    // The run the keys start finds the warehouse there and asks for its tables.
    let run = answer_each(&w.gate, vec![OK, OK]);
    let (status, body) = call(&w.app, put, Some((&admin, true)), Some(pasted)).await;
    assert_eq!((status, body["set"].clone()), (StatusCode::OK, json!(true)));
    assert!(!body.to_string().contains(&secret));
    for _ in 0..50 {
        if run.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(run.join().unwrap().len(), 2);
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT lake, access_key FROM lake_admin_keys")
            .fetch_all(&w.pool)
            .await
            .unwrap();
    assert_eq!(rows, [("pool:main".into(), "qd-platform-admin".into())]);
    let audited: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM admin_audit_events WHERE resource = 'lake/pool:main' \
         AND action = 'lake:admin-keys'",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(audited.0, 1);
    let (_, body) = call(
        &w.app,
        ("GET", "/lake/replica/admin-keys"),
        Some((&admin, false)),
        None,
    )
    .await;
    assert_eq!(body["set"], json!(false), "the replica's are apart");
    let info = r#"{"info":{"backend":{"onlineDisks":4,"offlineDisks":0}}}"#;
    let seen = answer_once(&w.gate, info);
    let (_, body) = call(&w.app, state, Some((&admin, false)), None).await;
    assert_eq!(
        (body["state"].as_str(), body["drives_online"].as_u64()),
        (Some("answered"), Some(4))
    );
    let head = seen.join().unwrap();
    assert!(head.starts_with("GET /rustfs/admin/v3/info "), "{head}");
    assert!(!head.contains(&secret) && !body.to_string().contains(&secret));
}

/// `/lake/replication` once its first warehouse is in `state`.
async fn replication_once(w: &World, admin: &Client, state: &str) -> Value {
    let route = ("GET", "/lake/replication");
    let mut body = Value::Null;
    for _ in 0..50 {
        body = call(&w.app, route, Some((admin, false)), None).await.1;
        if body["warehouses"][0]["state"] == json!(state) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    body
}

const OK: (&str, &str) = ("200 OK", "");

#[tokio::test]
async fn the_main_lakes_keys_make_the_warehouse_and_the_replicas_replicate_it() {
    let w = world(false, 100).await;
    let admin = sign_in(&w.app, ADMIN).await;
    let route = ("GET", "/lake/replication");
    let (_, body) = call(&w.app, route, Some((&admin, false)), None).await;
    assert_eq!(body["readiness"], json!("no_replica"));
    assert_eq!(
        (
            body["retention_days"].clone(),
            body["noncurrent_days"].clone()
        ),
        (json!(30), json!(7))
    );
    let pg = PgMachines::new(w.pool.clone());
    let made = auth_gateway::MachineRegistry::create_approved(
        &pg,
        "localhost",
        MachineRole::LakeReplica,
        SystemTime::now(),
    );
    made.await.unwrap().unwrap();
    let paste = |lake: &'static str, secret: String| {
        let body = json!({ "access_key": "qd-platform-admin", "secret_key": secret });
        call(&w.app, ("PUT", lake), Some((&admin, true)), Some(body))
    };
    // The main lake has no such warehouse: it is made, bucket then tables.
    let seen = answer_each(&w.gate, vec![("404 Not Found", ""), OK, OK]);
    let (status, _) = paste("/lake/main/admin-keys", "1a".repeat(32)).await;
    assert_eq!(status, StatusCode::OK);
    let body = replication_once(&w, &admin, "made").await;
    assert_eq!(
        (body["readiness"].clone(), body["lake"].clone()),
        (json!("no_keys"), json!("replica")),
        "{body}"
    );
    assert_eq!(body["warehouses"][0]["warehouse"], json!(WAREHOUSE));
    let lines: Vec<String> = seen.join().unwrap().iter().map(|h| first(h)).collect();
    assert_eq!(
        lines,
        [
            format!("HEAD /{WAREHOUSE}"),
            format!("PUT /{WAREHOUSE}"),
            format!("PUT /iceberg/v1/buckets/{WAREHOUSE}"),
        ]
    );
    // Both lakes are the stand-in: the warehouse is there, then replicated.
    let arn = "\"arn:minio:replication:us-east-1:d1:qd\"";
    let mut answers = vec![OK; 8];
    answers.insert(7, ("200 OK", arn));
    let seen = answer_each(&w.gate, answers);
    paste("/lake/replica/admin-keys", "2b".repeat(32)).await;
    let body = replication_once(&w, &admin, "configured").await;
    assert_eq!(body["readiness"], json!("ready"), "{body}");
    assert_eq!(body["warehouses"][0]["state"], json!("configured"));
    let port = w.gate.local_addr().unwrap().port();
    assert_eq!(body["proxy"], json!("localhost:9100"));
    assert_eq!(body["source"], json!(format!("http://localhost:{port}")));
    let heads = seen.join().unwrap();
    assert_eq!(heads.len(), 9);
    assert_eq!(
        first(&heads[1]),
        format!("PUT /iceberg/v1/buckets/{WAREHOUSE}")
    );
    assert!(!body.to_string().contains(&"2b".repeat(32)));
    let audited: Vec<(String, String)> = sqlx::query_as(
        "SELECT action, resource FROM admin_audit_events WHERE action LIKE 'lake:%:%' \
         AND action <> 'lake:admin-keys' ORDER BY occurred_at",
    )
    .fetch_all(&w.pool)
    .await
    .unwrap();
    assert_eq!(
        audited,
        [
            (
                "lake:warehouse:made".to_string(),
                format!("lake/pool:main/{WAREHOUSE}")
            ),
            (
                "lake:replication:configured".to_string(),
                format!("lake/replica/{WAREHOUSE}")
            ),
        ]
    );
}

/// A request's method and path.
fn first(head: &str) -> String {
    let line = head.lines().next().unwrap_or_default();
    line.rsplit_once(' ').map_or(line, |(l, _)| l).to_string()
}
