//! What a new machine is installed with (0029, 0066 step 11), end to end:
//! the real routes and use cases on PostgreSQL, the files embedded at build
//! and a CA made in the test. The script and the CA are anyone's, with the
//! hashes the join command holds; the role's compose file is given only for
//! a usable join token of that role, which then still joins. Nothing of the
//! token is answered or logged. Needs Docker.

use std::sync::Arc;
use std::time::Duration;

mod machine_certs;

use application::MachinesService;
use auth_gateway::MachineRole;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::ca::{Ca, DEFAULT_CA_VALIDITY};
use infrastructure::machines::{
    sha256_hex, JOIN_SCRIPT, LAKE_GATE_ENTRYPOINT, LAKE_REPLICA_ALLOY, LAKE_REPLICA_COMPOSE,
    RUSTFS_TLS_ENTRYPOINT, WORKER_ALLOY, WORKER_COMPOSE,
};
use infrastructure::{
    EmbeddedInstallFiles, PgAuditRepository, PgMachines, RingJoinTokenMint, SystemClock,
};
use machine_certs::{admin, csr, key, origin};
use serde_json::json;
use tower::ServiceExt;

/// A response's status, its SHA-256 header and its body.
type Answer = (StatusCode, String, String);

async fn get(app: &axum::Router, uri: &str, auth: Option<&str>) -> Answer {
    let mut request = Request::get(uri).header("x-forwarded-for", "203.0.113.9");
    if let Some(auth) = auth {
        request = request.header("authorization", auth);
    }
    let response = app.clone().oneshot(request.body(Body::empty()).unwrap());
    let response = response.await.unwrap();
    let status = response.status();
    let sha = response.headers().get("x-content-sha256");
    let sha = sha
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await;
    (
        status,
        sha,
        String::from_utf8(bytes.unwrap().to_vec()).unwrap(),
    )
}

async fn join(app: &axum::Router, token: &str, name: &str) -> StatusCode {
    let body = json!({ "token": token, "name": name, "csr": csr(&key()) });
    let request = Request::post("/install/join")
        .header("x-forwarded-for", "203.0.113.9")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap().status()
}

#[test]
fn a_machine_gets_its_files_and_its_compose_only_for_a_usable_token() {
    let mut tokens = Vec::new();
    let lines = logging::json_lines("server", || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        tokens = runtime.block_on(scenario());
    });
    assert!(lines.len() >= 14, "a line per request: {}", lines.len());
    let logged = serde_json::to_string(&lines).unwrap();
    for token in &tokens {
        assert!(!logged.contains(token.as_str()), "a token was logged");
        assert!(!logged.contains(&token[4..]), "part of a token was logged");
    }
}

/// The whole scenario; the tokens it used, to look for in the log.
async fn scenario() -> Vec<String> {
    let db = testkit::spawn_test_db().await;
    let made = Ca::create("test", DEFAULT_CA_VALIDITY).unwrap();
    let pg = Arc::new(PgMachines::new(db.pool.clone()));
    let files = EmbeddedInstallFiles::new(Some(&made.cert_pem)).unwrap();
    let machines = MachinesService::new(
        pg.clone(),
        Arc::new(RingJoinTokenMint::new()),
        Arc::new(PgAuditRepository::new(db.pool.clone())),
        Arc::new(SystemClock),
    )
    .with_signer(Arc::new(Ca::load(&made.cert_pem, &made.key_pem).unwrap()))
    .with_install(Arc::new(files), pg, "https://qd.example");
    let limit = api::rate_limit::RateLimitConfig {
        max_requests: 100,
        window: Duration::from_secs(60),
    };
    let app = api::machines::install::routes(machines.clone(), limit)
        .merge(api::machines::join::routes(machines.clone(), limit))
        .layer(axum::middleware::from_fn(api::telemetry::request_context));
    let issue = |role| {
        let machines = machines.clone();
        async move {
            let (by, from) = (admin(), origin());
            let issued = machines.issue_join_token(role, None, &by, &from);
            issued.await.unwrap()
        }
    };
    let issued = issue(MachineRole::Worker).await;
    let command = machines
        .join_command(&issued, &application::Drives::default())
        .unwrap();

    // The script and the CA, for anyone, with the command's hashes.
    let (status, sha, script) = get(&app, "/install/join.sh", None).await;
    assert_eq!((status, script.as_str()), (StatusCode::OK, JOIN_SCRIPT));
    assert_eq!(sha, sha256_hex(script.as_bytes()));
    assert_eq!(sha, command.script_sha256);
    let (status, sha, ca) = get(&app, "/install/ca.crt", None).await;
    assert_eq!(
        (status, ca.as_str()),
        (StatusCode::OK, made.cert_pem.as_str())
    );
    assert_eq!(sha, command.ca_sha256);
    // The fingerprint the node agent checks the join's CA against.
    assert_eq!(sha, node_agent::cert::fingerprint(&ca).unwrap());
    assert!(command.command.contains(&format!("--ca-sha256 '{sha}'")));
    assert!(command
        .command
        .contains("'https://qd.example/api/install/join.sh'"));

    // The compose file: none without a usable token of its role.
    let compose = "/install/worker/compose.yml";
    let bearer = |t: &str| format!("Bearer {t}");
    let unknown = format!("qdj_{}", "A".repeat(43));
    let other = issue(MachineRole::TrinoWorker).await;
    for auth in [
        None,
        Some(issued.token.clone()),
        Some(bearer("garbage")),
        Some(bearer(&unknown)),
        Some(bearer(&other.token)),
    ] {
        let (status, sha, body) = get(&app, compose, auth.as_deref()).await;
        assert_eq!((status, sha.as_str()), (StatusCode::UNAUTHORIZED, ""));
        assert!(!body.contains(&issued.token[4..]) && !body.contains(&other.token[4..]));
    }
    let own = "/install/trino-worker/compose.yml";
    let (status, ..) = get(&app, own, Some(&bearer(&other.token))).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no compose file for it yet");
    let (status, ..) = get(&app, "/install/admin/compose.yml", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The lake replica's files, for its own token alone.
    let replica = issue(MachineRole::LakeReplica).await;
    replica_files(&app, &replica.token, &issued.token).await;

    // With the token, twice: given, not used up.
    for _ in 0..2 {
        let (status, sha, body) = get(&app, compose, Some(&bearer(&issued.token))).await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, WORKER_COMPOSE));
        assert_eq!(sha, sha256_hex(WORKER_COMPOSE.as_bytes()));
        assert!(!body.contains(&issued.token[4..]));
    }
    // The machine still joins with it, once; then the compose file is refused.
    assert_eq!(
        join(&app, &issued.token, "worker-7").await,
        StatusCode::CREATED
    );
    let (status, ..) = get(&app, compose, Some(&bearer(&issued.token))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // An expired one, too.
    sqlx::query("UPDATE join_tokens SET expires_at = now() - interval '1 second'")
        .execute(&db.pool)
        .await
        .unwrap();
    let late = bearer(&other.token);
    assert_eq!(
        get(&app, own, Some(&late)).await.0,
        StatusCode::UNAUTHORIZED
    );
    vec![issued.token, other.token, replica.token]
}

/// The lake replica's compose file and the entrypoints it names, for its
/// own token alone (0066 step 11, ruling 4); none of another name or role.
async fn replica_files(app: &axum::Router, replica: &str, worker: &str) {
    let bearer = |t: &str| format!("Bearer {t}");
    let with = bearer(replica);
    for (uri, file) in [
        ("/install/lake-replica/compose.yml", LAKE_REPLICA_COMPOSE),
        (
            "/install/lake-replica/files/rustfs-tls.sh",
            RUSTFS_TLS_ENTRYPOINT,
        ),
        (
            "/install/lake-replica/files/lake-gate.sh",
            LAKE_GATE_ENTRYPOINT,
        ),
        (
            "/install/lake-replica/files/lake-replica.alloy",
            LAKE_REPLICA_ALLOY,
        ),
    ] {
        let (status, sha, body) = get(app, uri, Some(&with)).await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, file), "{uri}");
        assert_eq!(sha, sha256_hex(file.as_bytes()));
        let (status, ..) = get(app, uri, Some(&bearer(worker))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "a worker's token: {uri}");
    }
    let other = get(app, "/install/lake-replica/files/join.sh", Some(&with));
    assert_eq!(other.await.0, StatusCode::NOT_FOUND);
    let worker_file = "/install/worker/files/rustfs-tls.sh";
    let as_worker = bearer(worker);
    let other = get(app, worker_file, Some(&as_worker));
    assert_eq!(other.await.0, StatusCode::NOT_FOUND);
    // The worker's Alloy configuration (0071 step 4), for its token alone.
    let alloy = "/install/worker/files/worker.alloy";
    let (status, sha, body) = get(app, alloy, Some(&as_worker)).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, WORKER_ALLOY));
    assert_eq!(sha, sha256_hex(WORKER_ALLOY.as_bytes()));
    assert_eq!(
        get(app, alloy, Some(&with)).await.0,
        StatusCode::UNAUTHORIZED
    );
    let other = get(app, "/install/lake-replica/files/worker.alloy", Some(&with));
    assert_eq!(other.await.0, StatusCode::NOT_FOUND);
}

/// The routes that check a token have a limit of their own (0066 step 11,
/// the user's ruling of 2026-11-13): a replica's run (four requests) passes
/// at four, the fifth is refused, and neither `join.sh` nor joining, with a
/// limit of its own, is held by it.
#[tokio::test]
async fn the_install_routes_are_limited_on_their_own() {
    let db = testkit::spawn_test_db().await;
    let made = Ca::create("test", DEFAULT_CA_VALIDITY).unwrap();
    let pg = Arc::new(PgMachines::new(db.pool.clone()));
    let files = EmbeddedInstallFiles::new(Some(&made.cert_pem)).unwrap();
    let machines = MachinesService::new(
        pg.clone(),
        Arc::new(RingJoinTokenMint::new()),
        Arc::new(PgAuditRepository::new(db.pool.clone())),
        Arc::new(SystemClock),
    )
    .with_signer(Arc::new(Ca::load(&made.cert_pem, &made.key_pem).unwrap()))
    .with_install(Arc::new(files), pg, "https://qd.example");
    let limit = |max_requests| api::rate_limit::RateLimitConfig {
        max_requests,
        window: Duration::from_secs(60),
    };
    let app = api::machines::install::routes(machines.clone(), limit(4))
        .merge(api::machines::join::routes(machines.clone(), limit(1)));
    let (by, from) = (admin(), origin());
    let replica = machines.issue_join_token(MachineRole::LakeReplica, None, &by, &from);
    let auth = format!("Bearer {}", replica.await.unwrap().token);
    let run = [
        "/install/lake-replica/compose.yml",
        "/install/lake-replica/files/rustfs-tls.sh",
        "/install/lake-replica/files/lake-gate.sh",
        "/install/lake-replica/compose.yml",
    ];
    for uri in run {
        assert_eq!(get(&app, uri, Some(&auth)).await.0, StatusCode::OK, "{uri}");
    }
    let (status, _, body) = get(&app, run[0], Some(&auth)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(body.contains("too_many_attempts"), "{body}");
    assert_eq!(get(&app, "/install/join.sh", None).await.0, StatusCode::OK);
    let token = auth.trim_start_matches("Bearer ");
    assert_eq!(join(&app, token, "replica-1").await, StatusCode::CREATED);
}
