//! Settings → Lake's pools (0066 step 11, the user's rulings 1 to 3 of
//! 2026-11-14), end to end: the real routes and use cases on PostgreSQL;
//! `main` defined before any node, a node's command naming it, and joining
//! under another name refused. Needs Docker.

mod machines_world;

use axum::http::StatusCode;
use machines_world::*;
use serde_json::json;

#[tokio::test]
async fn main_is_defined_before_any_node_by_whoever_may_manage_machines() {
    let w = world(true, 100).await;
    let admin = sign_in(&w.app, ADMIN).await;
    let user = sign_in(&w.app, USER).await;
    let (list, define) = (("GET", "/lake/pools"), ("PUT", "/lake/pools/main"));
    let four = json!({ "nodes": ["lake-a", "lake-b", "lake-c", "lake-d"], "drives": 2 });
    let (status, body) = call(&w.app, list, Some((&admin, false)), None).await;
    assert_eq!((status, body), (StatusCode::OK, json!({ "pools": [] })));
    let (status, _) = call(&w.app, list, Some((&user, false)), None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "without admin:ManageMachines"
    );
    let (status, body) = call(&w.app, define, Some((&admin, false)), Some(four.clone())).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("csrf_mismatch"))
    );
    for (uri, body, field) in [
        (
            "/lake/pools/main",
            json!({ "nodes": [], "drives": 1 }),
            "nodes",
        ),
        (
            "/lake/pools/main",
            json!({ "nodes": ["a-", "b", "c", "d"], "drives": 1 }),
            "nodes",
        ),
        (
            "/lake/pools/main",
            json!({ "nodes": ["a", "b", "c", "d"], "drives": 0 }),
            "drives",
        ),
        ("/lake/pools/second", four.clone(), "name"),
    ] {
        let (status, refused) = call(&w.app, ("PUT", uri), Some((&admin, true)), Some(body)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
        assert!(refused["fields"][field].is_string(), "{refused}");
    }

    let (status, pool) = call(&w.app, define, Some((&admin, true)), Some(four)).await;
    assert_eq!(
        (status, pool["name"].as_str(), pool["drives"].as_u64()),
        (StatusCode::OK, Some("main"), Some(2))
    );
    assert_eq!(
        pool["nodes"][1],
        json!({ "name": "lake-b", "state": null, "machine_id": null })
    );
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM admin_audit_events WHERE action = 'lake:pool:defined' AND resource = 'lake/pool:main'",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(audited, 1);
}

#[tokio::test]
async fn a_nodes_command_names_it_and_another_name_is_refused() {
    let w = world(true, 100).await;
    let admin = sign_in(&w.app, ADMIN).await;
    let (list, define) = (("GET", "/lake/pools"), ("PUT", "/lake/pools/main"));
    let four = json!({ "nodes": ["lake-a", "lake-b", "lake-c", "lake-d"], "drives": 2 });
    let (status, _) = call(&w.app, define, Some((&admin, true)), Some(four)).await;
    assert_eq!(status, StatusCode::OK);
    let issue = ("POST", "/machines/join-tokens");
    let ask = |node: &str, drives: &[&str]| {
        Some(json!({ "role": "lake", "pool": "main", "node": node, "drives": drives }))
    };
    for (body, field) in [
        (ask("lake-z", &[]), "node"),
        (ask("lake-b", &["/srv/d1"]), "drives"),
        (
            Some(json!({ "role": "lake", "pool": "other", "node": "lake-b" })),
            "pool",
        ),
        (Some(json!({ "role": "worker", "node": "lake-b" })), "node"),
    ] {
        let (status, refused) = call(&w.app, issue, Some((&admin, true)), body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
        assert!(refused["fields"][field].is_string(), "{refused}");
    }
    let (status, token) = call(
        &w.app,
        issue,
        Some((&admin, true)),
        ask("lake-b", &["/srv/d1", "/srv/d2"]),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{token}");
    assert_eq!(
        (token["role"].as_str(), token["node"].as_str()),
        (Some("lake"), Some("lake-b"))
    );
    let secret = token["token"].as_str().unwrap();

    let (status, body) = join(&w, secret, "lake-a", &good_csr()).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("node_name"))
    );
    let (status, _) = join(&w, secret, "lake-b", &good_csr()).await;
    assert_eq!(status, StatusCode::CREATED, "the token was left unused");
    let (_, pools) = call(&w.app, list, Some((&admin, false)), None).await;
    let nodes = &pools["pools"][0]["nodes"];
    assert_eq!(
        (nodes[0]["state"].clone(), nodes[1]["state"].clone()),
        (json!(null), json!("pending"))
    );
    let again = json!({ "nodes": ["lake-a", "lake-b", "lake-c", "lake-e"], "drives": 2 });
    let (status, body) = call(&w.app, define, Some((&admin, true)), Some(again)).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("pool_joined"))
    );
}
