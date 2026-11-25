//! What a lake pool's node is installed with (0066 step 11, the user's
//! rulings 1, 2 and 4 of 2026-11-14), end to end on PostgreSQL: for a usable
//! token of the role `lake` that names the node, its compose file, the
//! pool's layout (`pool.env`) and the lake's root keys, kept under
//! `pool:main`, the same for every node. Needs Docker.

use std::sync::Arc;
use std::time::Duration;

mod machine_certs;

use application::MachinesService;
use auth_gateway::MachineRole;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use infrastructure::ca::{Ca, DEFAULT_CA_VALIDITY};
use infrastructure::{
    EmbeddedInstallFiles, PgAuditRepository, PgLakePools, PgLakeRootKeys, PgMachines,
    RingJoinTokenMint, SecretBox, SystemClock,
};
use machine_certs::{admin, origin};
use tower::ServiceExt;

async fn get(app: &axum::Router, uri: &str, token: &str) -> (StatusCode, String) {
    let request = Request::get(uri)
        .header("x-forwarded-for", "203.0.113.9")
        .header("authorization", format!("Bearer {token}"));
    let response = app.clone().oneshot(request.body(Body::empty()).unwrap());
    let response = response.await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await;
    (status, String::from_utf8(bytes.unwrap().to_vec()).unwrap())
}

/// The service and the install routes over the test database.
fn install(db: &testkit::TestDb) -> (MachinesService, axum::Router) {
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
    .with_lake_keys(Arc::new(PgLakeRootKeys::new(db.pool.clone(), sealer)))
    .with_lake_pools(Arc::new(PgLakePools::new(db.pool.clone())));
    let limit = api::rate_limit::RateLimitConfig {
        max_requests: 100,
        window: Duration::from_secs(60),
    };
    let app = api::machines::install::routes(machines.clone(), limit);
    (machines, app)
}

#[tokio::test]
async fn a_pool_node_gets_its_layout_and_the_lakes_root_keys_for_its_token() {
    let db = testkit::spawn_test_db().await;
    let (machines, app) = install(&db);
    let (by, from) = (admin(), origin());
    let nodes: Vec<String> = ["lake-a", "lake-b", "lake-c", "lake-d"]
        .map(String::from)
        .to_vec();
    machines
        .define_pool("main", &nodes, 2, &by, &from)
        .await
        .unwrap();
    let node = |name: &'static str| {
        let machines = machines.clone();
        let (by, from) = (by.clone(), from.clone());
        async move {
            machines
                .issue_node_token("main", name, 0, &by, &from)
                .await
                .unwrap()
        }
    };
    let (b, c) = (node("lake-b").await, node("lake-c").await);
    let worker = machines
        .issue_join_token(MachineRole::Worker, None, &by, &from)
        .await;
    let worker = worker.unwrap();

    let (status, compose) = get(&app, "/install/lake/compose.yml", &b.token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(compose.contains("name: qd-lake"));
    let (status, proxy) = get(&app, "/install/lake/files/lake-proxy.sh", &b.token).await;
    assert_eq!(
        (status, proxy.contains("QD_LAKE_LISTEN_TLS")),
        (StatusCode::OK, true)
    );
    let (status, alloy) = get(&app, "/install/lake/files/lake.alloy", &b.token).await;
    assert_eq!(
        (status, alloy.contains("role = \"lake\"")),
        (StatusCode::OK, true)
    );
    let (status, env) = get(&app, "/install/lake/pool.env", &b.token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        env.starts_with("QD_POOL=main\nQD_POOL_NODE=lake-b\n"),
        "{env}"
    );
    assert!(
        env.contains("\nRUSTFS_VOLUMES=\"https://lake-a:9010/data/1 https://lake-a:9010/data/2 ")
    );
    assert!(!env.contains("QD_LAKE_REPLICA_GATE"), "no replica approved");
    let (_, other) = get(&app, "/install/lake/pool.env", &c.token).await;
    assert!(other.contains("QD_POOL_NODE=lake-c"));
    for (uri, token) in [
        ("/install/lake/pool.env", worker.token.as_str()),
        ("/install/lake/root-keys.env", worker.token.as_str()),
        ("/install/worker/pool.env", "garbage"),
    ] {
        assert_eq!(
            get(&app, uri, token).await.0,
            StatusCode::UNAUTHORIZED,
            "{uri}"
        );
    }
    assert_eq!(
        get(&app, "/install/worker/pool.env", &worker.token).await.0,
        StatusCode::NOT_FOUND
    );

    // The lake's root keys, the same for both nodes, kept once as pool:main.
    let (status, keys_b) = get(&app, "/install/lake/root-keys.env", &b.token).await;
    assert_eq!(status, StatusCode::OK);
    let (_, keys_c) = get(&app, "/install/lake/root-keys.env", &c.token).await;
    assert_eq!(keys_b, keys_c);
    let lakes: Vec<String> = sqlx::query_scalar("SELECT lake FROM lake_root_keys")
        .fetch_all(&db.pool)
        .await
        .unwrap();
    assert_eq!(lakes, ["pool:main"]);
}

/// The `pool.env` a lone node `lake-a` of `drives` drives gets.
async fn one_node_env(drives: u16) -> String {
    let db = testkit::spawn_test_db().await;
    let (machines, app) = install(&db);
    let (by, from) = (admin(), origin());
    let one = vec!["lake-a".to_string()];
    machines
        .define_pool("main", &one, drives, &by, &from)
        .await
        .unwrap();
    let token = machines
        .issue_node_token("main", "lake-a", 0, &by, &from)
        .await
        .unwrap();
    let (status, env) = get(&app, "/install/lake/pool.env", &token.token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(env.contains("QD_POOL_NODES=\"lake-a\"\n"), "{env}");
    env
}

/// A lake on one node (decision 0091, point 3): one drive by path, as
/// RustFS asks of a single drive, or four by URL, so it can take a pool.
#[tokio::test]
async fn a_one_node_pool_gets_one_drive_by_path_or_several_by_url() {
    let one = one_node_env(1).await;
    assert!(one.contains("\nRUSTFS_VOLUMES=\"/data/1\"\n"), "{one}");
    let four = one_node_env(4).await;
    let urls: Vec<String> = (1..=4)
        .map(|n| format!("https://lake-a:9010/data/{n}"))
        .collect();
    let volumes = format!("\nRUSTFS_VOLUMES=\"{}\"\n", urls.join(" "));
    assert!(four.contains(&volumes), "{four}");
}
