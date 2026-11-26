//! The generic `lake-admin/1` client against the in-memory lake served in
//! process with mTLS (decision 0078, step 6): each operation, a refused
//! certificate both ways, and what is tried again.

use std::sync::Arc;
use std::time::Duration;

use application::lake::{
    LakeAdmin, LakeCredentialKind, LakeError, LakeGrant, LakeHealth, ManagedAccess, ManagedPolicy,
    ReplicaTarget, ReplicationState, RetentionRules, WarehouseMade,
};
use infrastructure::lake_admin_client::LakeAdminClient;
use testkit::lake::InMemoryLake;
use testkit::lake_admin::{serve, serve_as, CALLER_ROLE};
use testkit::tls::TestCa;

const WH: &str = "qd-tenant-00000000000000000000000000000001";

fn client(url: &str, ca: &TestCa, role: &str) -> LakeAdminClient {
    LakeAdminClient::new(url, &ca.pem, &ca.machine(role, "app-1").bundle())
        .unwrap()
        .pausing(Duration::from_millis(10))
}

#[tokio::test]
async fn each_operation_reaches_the_lake() {
    let ca = TestCa::new();
    let server = serve(Arc::new(InMemoryLake::default()), &ca).await;
    let admin = client(&server.url, &ca, CALLER_ROLE);
    let rules = RetentionRules {
        versioning: true,
        locked_days: Some(30),
        noncurrent_expire_days: None,
    };
    let made = admin.ensure_warehouse(WH, &rules).await;
    assert_eq!(made, Ok(WarehouseMade::Made));
    assert_eq!(server.lake.snapshot().warehouses[WH], rules);
    let again = admin.ensure_warehouse(WH, &rules).await;
    assert_eq!(again, Ok(WarehouseMade::Present));
    let replica = ReplicaTarget {
        endpoint: "https://replica.test".into(),
        source_pool: true,
    };
    assert_eq!(
        admin.replicate(WH, &replica).await.unwrap(),
        ReplicationState::Configured
    );
    let sync = admin.replication(WH).await.unwrap();
    assert_eq!(
        (sync.state, sync.pending, sync.failed),
        (ReplicationState::Configured, Some(0), Some(0))
    );
    let policies = [ManagedPolicy {
        name: "quality-viewer".into(),
        access: ManagedAccess::ReadUnder("reports/".into()),
    }];
    admin.set_managed_policies(&policies).await.unwrap();
    assert_eq!(server.lake.snapshot().managed, policies);
    let policy = admin
        .session_policy(&LakeGrant::read_only(WH))
        .await
        .unwrap();
    assert!(!policy.0.is_empty());
    let made = admin
        .ensure_credentials(LakeCredentialKind::Admin, None)
        .await
        .unwrap();
    let kept = admin
        .ensure_credentials(LakeCredentialKind::Admin, Some(&made))
        .await;
    assert_eq!(kept.unwrap(), made);
    let about = admin.about().await.unwrap();
    assert_eq!(about.protocols, [contracts::lake_admin::PROTOCOL]);
    let health = admin.health().await.unwrap();
    assert!(matches!(health, LakeHealth::Answered { .. }), "{health:?}");
    let unknown = admin.replication("qd-tenant-unknown").await;
    assert!(matches!(unknown, Err(LakeError::Unknown(_))), "{unknown:?}");
    let invalid = admin.session_policy(&LakeGrant::read_only("Bad*")).await;
    assert!(matches!(invalid, Err(LakeError::Invalid(_))), "{invalid:?}");
}

#[tokio::test]
async fn a_refused_certificate_is_refused_both_ways() {
    let ca = TestCa::new();
    let server = serve(Arc::new(InMemoryLake::default()), &ca).await;
    let stranger = client(&server.url, &ca, "worker");
    let refused = stranger.health().await;
    assert!(matches!(refused, Err(LakeError::Refused(_))), "{refused:?}");

    let not_an_adapter = serve_as(Arc::new(InMemoryLake::default()), &ca, "worker").await;
    let admin = client(&not_an_adapter.url, &ca, CALLER_ROLE);
    let refused = admin.health().await;
    assert!(matches!(refused, Err(LakeError::Refused(_))), "{refused:?}");
}

#[tokio::test]
async fn an_adapter_that_does_not_answer_is_tried_again_then_unavailable() {
    let ca = TestCa::new();
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("https://localhost:{}", closed.local_addr().unwrap().port());
    drop(closed);
    let started = std::time::Instant::now();
    let answer = client(&url, &ca, CALLER_ROLE).health().await;
    assert!(
        matches!(answer, Err(LakeError::Unavailable(_))),
        "{answer:?}"
    );
    // Three attempts, paused 10 ms then 20 ms between them.
    assert!(started.elapsed() >= Duration::from_millis(30));
}

/// The main lake's admin keys, kept.
struct Kept;

#[async_trait::async_trait]
impl infrastructure::lake_admin_client::AdminKeySource for Kept {
    async fn open(
        &self,
        _: &auth_gateway::Lake,
    ) -> Result<Option<auth_gateway::LakeAdminKeys>, domain::RepositoryError> {
        Ok(Some(auth_gateway::LakeAdminKeys {
            access_key: "qd-admin".into(),
            secret_key: "the-admin-secret".into(),
        }))
    }
}

#[tokio::test]
async fn a_user_with_given_keys_is_made_with_the_lakes_admin_keys() {
    let ca = TestCa::new();
    let server = serve(Arc::new(InMemoryLake::default()), &ca).await;
    let given = application::lake::LakeKeys {
        access_key: "trino-coordinator".into(),
        secret_key: "a-secret-the-agent-made-0123456789".into(),
    };
    let without = client(&server.url, &ca, CALLER_ROLE);
    let refused = without
        .ensure_credentials(LakeCredentialKind::QueryEngine, Some(&given))
        .await;
    assert!(matches!(refused, Err(LakeError::Invalid(_))), "{refused:?}");
    let admin =
        client(&server.url, &ca, CALLER_ROLE).with_keys(Arc::new(Kept), auth_gateway::Lake::main());
    for kind in [
        LakeCredentialKind::QueryEngine,
        LakeCredentialKind::StsIssuer,
    ] {
        let made = admin.ensure_credentials(kind, Some(&given)).await;
        assert_eq!(made.unwrap(), given, "{kind:?}");
    }
}
