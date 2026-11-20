//! A lake's root keys for a node that is installing (0066 step 11, ruling
//! 1), end to end: the real route and use case on PostgreSQL, the keys made
//! once and kept sealed with the settings' key, given for a usable token of
//! the replica's role alone, audited each time, never logged. Needs Docker.

use std::sync::Arc;
use std::time::Duration;

mod machine_certs;

use application::MachinesService;
use auth_gateway::MachineRole;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::ca::{Ca, DEFAULT_CA_VALIDITY};
use infrastructure::{
    EmbeddedInstallFiles, PgAuditRepository, PgLakeRootKeys, PgMachines, RingJoinTokenMint,
    SecretBox, SystemClock,
};
use machine_certs::{admin, origin};
use tower::ServiceExt;

const KEYS: &str = "/install/lake-replica/root-keys.env";

/// A response's status, its cache and hash headers, and its body.
async fn get(app: &axum::Router, uri: &str, token: Option<&str>) -> (StatusCode, String, String) {
    let mut request = Request::get(uri).header("x-forwarded-for", "203.0.113.9");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app.clone().oneshot(request.body(Body::empty()).unwrap());
    let response = response.await.unwrap();
    let status = response.status();
    let header = |name: &str| {
        let value = response.headers().get(name);
        value
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default()
    };
    let headers = format!("{}|{}", header("cache-control"), header("x-content-sha256"));
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await;
    let body = String::from_utf8(bytes.unwrap().to_vec()).unwrap();
    (status, headers, body)
}

#[test]
fn the_replica_gets_its_root_keys_only_for_its_token_sealed_and_audited() {
    let mut secrets = Vec::new();
    let lines = logging::json_lines("server", || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        secrets = runtime.block_on(scenario());
    });
    assert!(lines.len() >= 6, "a line per request: {}", lines.len());
    let logged = serde_json::to_string(&lines).unwrap();
    for secret in &secrets {
        assert!(!logged.contains(secret.as_str()), "a secret was logged");
    }
}

/// The whole scenario; the tokens and keys it saw, to look for in the log.
async fn scenario() -> Vec<String> {
    let db = testkit::spawn_test_db().await;
    let made = Ca::create("test", DEFAULT_CA_VALIDITY).unwrap();
    let pg = Arc::new(PgMachines::new(db.pool.clone()));
    let files = EmbeddedInstallFiles::new(Some(&made.cert_pem)).unwrap();
    let sealer = SecretBox::from_key(&[7; 32]).unwrap();
    let machines = MachinesService::new(
        pg.clone(),
        Arc::new(RingJoinTokenMint::new()),
        Arc::new(PgAuditRepository::new(db.pool.clone())),
        Arc::new(SystemClock),
    )
    .with_signer(Arc::new(Ca::load(&made.cert_pem, &made.key_pem).unwrap()))
    .with_install(Arc::new(files), pg, "https://qd.example")
    .with_lake_keys(Arc::new(PgLakeRootKeys::new(
        db.pool.clone(),
        sealer.clone(),
    )));
    let limit = api::rate_limit::RateLimitConfig {
        max_requests: 100,
        window: Duration::from_secs(60),
    };
    let app = api::machines::install::routes(machines.clone(), limit)
        .layer(axum::middleware::from_fn(api::telemetry::request_context));
    let issue = |role| {
        let machines = machines.clone();
        async move {
            let (by, from) = (admin(), origin());
            machines
                .issue_join_token(role, None, &by, &from)
                .await
                .unwrap()
        }
    };
    let replica = issue(MachineRole::LakeReplica).await;
    let worker = issue(MachineRole::Worker).await;

    // Refused without the replica's own token (a pool node's:
    // machines_install_pool_it.rs).
    for token in [None, Some("garbage"), Some(worker.token.as_str())] {
        let (status, _, body) = get(&app, KEYS, token).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(!body.contains("RUSTFS_"));
    }

    // Given twice, the same, no-store and without a hash.
    let (status, headers, first) = get(&app, KEYS, Some(&replica.token)).await;
    assert_eq!((status, headers.as_str()), (StatusCode::OK, "no-store|"));
    let (status, _, second) = get(&app, KEYS, Some(&replica.token)).await;
    assert_eq!((status, &second), (StatusCode::OK, &first));
    let pairs: Vec<(&str, &str)> = first.lines().map(|l| l.split_once('=').unwrap()).collect();
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].0, "RUSTFS_ACCESS_KEY");
    assert_eq!(pairs[1].0, "RUSTFS_SECRET_KEY");
    let (access, secret) = (pairs[0].1.to_string(), pairs[1].1.to_string());
    assert!(access.starts_with("qd") && secret.len() == 40);

    // One row, its secret sealed.
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT lake, access_key, secret_key FROM lake_root_keys")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].0.as_str(), &rows[0].1), ("replica", &access));
    assert_ne!(rows[0].2, secret, "sealed");
    assert_eq!(sealer.decrypt(&rows[0].2).unwrap(), secret);

    // Audited each time, from the node's address, without the keys.
    let events: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT event_type, action, resource, ip FROM admin_audit_events \
         WHERE event_type = 'credentials_issued'",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    let given = (
        "credentials_issued".to_string(),
        "install:lake-replica".to_string(),
        "lake/replica".to_string(),
        "203.0.113.9".to_string(),
    );
    assert_eq!(events, [given.clone(), given]);
    vec![replica.token, worker.token, access, secret]
}
