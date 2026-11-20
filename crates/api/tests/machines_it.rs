//! Settings → Execution's machines and the join route (0029, 0066 step 3),
//! end to end: the real routes, the real use cases, PostgreSQL, and a CA
//! and signing requests made in each test (no key is kept in the
//! repository). Needs Docker.

mod machines_world;

use application::Acceptance;
use auth_gateway::MachineRole;
use axum::http::StatusCode;
use infrastructure::ca::certificate_info;
use machines_world::*;
use serde_json::{json, Value};

#[tokio::test]
async fn machines_are_managed_with_the_action_and_csrf() {
    let w = world(true, 100).await;
    let admin = sign_in(&w.app, ADMIN).await;
    let user = sign_in(&w.app, USER).await;
    let issue = ("POST", "/machines/join-tokens");
    let worker = Some(json!({ "role": "worker", "pool": "default" }));

    let (status, _) = call(&w.app, ("GET", "/machines"), Some((&user, true)), None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "without admin:ManageMachines"
    );
    let (status, _) = call(&w.app, issue, Some((&user, true)), worker.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = call(&w.app, issue, Some((&admin, false)), worker.clone()).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("csrf_mismatch"))
    );
    let (status, _) = call(&w.app, ("GET", "/machines"), None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let bad_role = Some(json!({ "role": "admin" }));
    let (status, body) = call(&w.app, issue, Some((&admin, true)), bad_role).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["fields"]["role"].is_string());

    let (status, issued) = call(&w.app, issue, Some((&admin, true)), worker).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        (issued["role"].as_str(), issued["pool"].as_str()),
        (Some("worker"), Some("default"))
    );
    assert!(off(issued["expires_at"].as_i64().unwrap(), 3600) < 60);
    let token = issued["token"].as_str().unwrap();
    let (csr, origin) = (good_csr(), origin());
    let joined = w.machines.join(token, "worker-01", &csr, &origin);
    assert!(joined.await.is_ok());

    let (status, list) = call(&w.app, ("GET", "/machines"), Some((&admin, false)), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["can_join"], json!(true));
    let machine = &list["machines"][0];
    assert_eq!(
        (machine["name"].as_str(), machine["state"].as_str()),
        (Some("worker-01"), Some("pending"))
    );
    assert!(machine["cert_serial"].is_string());
    assert!(off(machine["cert_not_after"].as_i64().unwrap(), DAY) < 120);
    let id = machine["id"].as_str().unwrap();

    let to = |verb: &str| format!("/machines/{id}/{verb}");
    let client = Some((&admin, true));
    let drain = to("drain");
    let (status, body) = call(&w.app, ("POST", &drain), client, None).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("transition_not_allowed"))
    );
    for (verb, state) in [
        ("approve", "approved"),
        ("drain", "draining"),
        ("retire", "retired"),
    ] {
        let (status, body) = call(&w.app, ("POST", &to(verb)), client, None).await;
        assert_eq!(
            (status, body["state"].as_str()),
            (StatusCode::OK, Some(state)),
            "{verb}"
        );
    }
    let (status, body) = call(&w.app, ("POST", &to("retire")), client, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["message"]
        .as_str()
        .is_some_and(|m| m.contains("retired")));
    let unknown = "/machines/00000000-0000-0000-0000-000000000000/approve";
    let (status, _) = call(&w.app, ("POST", unknown), client, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn machines_get_no_join_token_without_a_ca() {
    let w = world(false, 100).await;
    let admin = sign_in(&w.app, ADMIN).await;
    let issue = ("POST", "/machines/join-tokens");
    let worker = Some(json!({ "role": "worker" }));
    let (status, body) = call(&w.app, issue, Some((&admin, true)), worker).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("no_ca"))
    );
    let (_, list) = call(&w.app, ("GET", "/machines"), Some((&admin, false)), None).await;
    assert_eq!(list["can_join"], json!(false));
}

#[tokio::test]
async fn machines_join_with_a_certificate_the_ca_writes() {
    let w = world(true, 100).await;
    let token = new_token(&w, MachineRole::Worker).await;
    let (status, body) = join(&w, &token, "worker-01", &good_csr()).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["state"], json!("pending"));
    let certificate = body["certificate"].as_str().unwrap();
    let info = certificate_info(certificate).unwrap();
    assert_eq!(info.organization.as_deref(), Some("qd"));
    assert_eq!(info.unit.as_deref(), Some("worker"));
    assert_eq!(info.common_name.as_deref(), Some("worker-01"));
    let serial = body["serial"].as_str().unwrap();
    assert_eq!(info.serial, serial);
    assert_eq!(
        info.not_after.timestamp(),
        body["not_after"].as_i64().unwrap()
    );
    assert!(
        off(info.not_after.timestamp(), DAY) < 120,
        "a worker: 24 hours"
    );
    assert!(body["ca"].as_str().unwrap().contains("BEGIN CERTIFICATE"));
    let accepted = w.machines.accepts("worker-01", serial).await.unwrap();
    assert!(
        matches!(accepted, Acceptance::Accepted(m) if m.cert_serial.as_deref() == Some(serial))
    );

    // A token is used once.
    let (status, used) = join(&w, &token, "worker-02", &good_csr()).await;
    assert_eq!(
        (status, used["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("join_refused"))
    );

    // Trino's coordinator takes its certificate only at a restart: a year.
    let trino = new_token(&w, MachineRole::TrinoCoordinator).await;
    let (status, body) = join(&w, &trino, "trino-1", &good_csr()).await;
    assert_eq!(status, StatusCode::CREATED);
    let info = certificate_info(body["certificate"].as_str().unwrap()).unwrap();
    assert_eq!(info.unit.as_deref(), Some("trino-coordinator"));
    assert!(off(info.not_after.timestamp(), 365 * DAY) < 120);
}

#[tokio::test]
async fn machines_are_refused_alike_and_a_bad_request_keeps_the_token() {
    let w = world(true, 100).await;
    let unknown = format!("qdj_{}", "A".repeat(43));
    let (status, unknown) = join(&w, &unknown, "worker-01", &good_csr()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let expired = new_token(&w, MachineRole::Worker).await;
    let past = "UPDATE join_tokens SET expires_at = now() - interval '1 second'";
    sqlx::query(past).execute(&w.pool).await.unwrap();
    let (status, body) = join(&w, &expired, "worker-01", &good_csr()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let same = |v: &Value| (v["code"].clone(), v["message"].clone());
    assert_eq!(same(&body), same(&unknown), "no oracle");

    let token = new_token(&w, MachineRole::Worker).await;
    let p384 = csr(&rcgen::PKCS_ECDSA_P384_SHA384);
    for bad in [p384.as_str(), "not a request"] {
        let (status, body) = join(&w, &token, "worker-01", bad).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body["fields"]["csr"].is_string());
    }
    let (status, body) = join(&w, &token, "Worker_01", &good_csr()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["fields"]["name"].is_string());
    let (status, _) = join(&w, &token, "worker-01", &good_csr()).await;
    assert_eq!(status, StatusCode::CREATED, "the token was still unused");

    let other = new_token(&w, MachineRole::Worker).await;
    let (status, body) = join(&w, &other, "worker-01", &good_csr()).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("name_taken"))
    );
    let (status, _) = join(&w, &other, "worker-02", &good_csr()).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a name taken leaves the token unused"
    );

    let huge = "A".repeat(api::machines::join::JOIN_BODY_LIMIT);
    let (status, _) = join(&w, &other, "worker-03", &huge).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn machines_cannot_join_without_a_ca_nor_too_often() {
    let w = world(false, 2).await;
    let token = format!("qdj_{}", "A".repeat(43));
    for _ in 0..2 {
        let (status, _) = join(&w, &token, "worker-01", &good_csr()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }
    let (status, body) = join(&w, &token, "worker-01", &good_csr()).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::TOO_MANY_REQUESTS, Some("too_many_attempts"))
    );
}

#[tokio::test]
async fn machines_give_a_replica_s_command_with_the_drives_given_or_none() {
    let w = world(true, 100).await;
    let admin = sign_in(&w.app, ADMIN).await;
    let issue = ("POST", "/machines/join-tokens");
    let client = Some((&admin, true));
    let replica = |drives: Value| Some(json!({ "role": "lake-replica", "drives": drives }));

    let given = replica(json!(["/srv/d1", "/srv/d2/"]));
    let (status, issued) = call(&w.app, issue, client, given).await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let command = issued["install"]["command"].as_str().unwrap();
    let line = "--role lake-replica --drive '/srv/d1' --drive '/srv/d2' \\\n";
    assert!(command.contains(line), "{command}");
    for none in [
        json!({ "role": "lake-replica" }),
        replica(json!([])).unwrap(),
    ] {
        let (status, issued) = call(&w.app, issue, client, Some(none)).await;
        assert_eq!(status, StatusCode::CREATED);
        let command = issued["install"]["command"].as_str().unwrap();
        assert!(command.contains("--role lake-replica \\\n"), "{command}");
        assert!(!command.contains("--drive"));
    }

    // Refused, with a message on the field, and no token issued.
    let tokens = "SELECT count(*) FROM join_tokens";
    let before: i64 = sqlx::query_scalar(tokens).fetch_one(&w.pool).await.unwrap();
    let many: Vec<String> = (0..=16).map(|n| format!("/srv/d{n}")).collect();
    let worker = Some(json!({ "role": "worker", "drives": ["/srv/d1"] }));
    for bad in [
        worker,
        replica(json!(["/srv/d1'; reboot; '"])),
        replica(json!(["srv/d1"])),
        replica(json!(["/srv/../etc"])),
        replica(json!(["/srv/d1", "/srv/d1"])),
        replica(json!(many)),
    ] {
        let (status, body) = call(&w.app, issue, client, bad.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad:?}");
        assert!(body["fields"]["drives"].is_string(), "{body}");
    }
    let after: i64 = sqlx::query_scalar(tokens).fetch_one(&w.pool).await.unwrap();
    assert_eq!(after, before);
}
