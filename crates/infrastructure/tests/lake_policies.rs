//! A grant's session policy, cached in front of the main lake's adapter
//! (decision 0078, point 9): asked once while it holds, asked again when
//! the adapter's version changes or the policy expires, and never served
//! expired while the adapter is down.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use application::lake::{
    LakeAdmin, LakeAdmins, LakeCredentialKind, LakeError, LakeGrant, LakeHealth, LakeKeys,
    ManagedPolicy, ReplicaTarget, ReplicationState, ReplicationSync, RetentionRules, SessionPolicy,
    WarehouseMade,
};
use async_trait::async_trait;
use auth_gateway::Lake;
use infrastructure::lake_policies::{AdapterVersion, SessionPolicies};
use testkit::lake::InMemoryLake;

const W: &str = "qd-tenant-00000000000000000000000000000001";

/// The main lake's adapter: the in-memory lake's translation, counted, and
/// down when said.
#[derive(Default)]
struct Adapter {
    lake: InMemoryLake,
    asked: AtomicU32,
    down: AtomicBool,
    version: Mutex<String>,
}

impl Adapter {
    fn answer(&self) -> Result<(), LakeError> {
        if self.down.load(Ordering::SeqCst) {
            return Err(LakeError::Unavailable("GET about: no connection".into()));
        }
        Ok(())
    }
}

struct Admins(Arc<Adapter>);

impl LakeAdmins for Admins {
    fn for_lake(&self, lake: &Lake, _: &str) -> Arc<dyn LakeAdmin> {
        assert_eq!(*lake, Lake::main(), "a policy is the main lake's");
        Arc::new(At(self.0.clone()))
    }
}

struct Version(Arc<Adapter>);

#[async_trait]
impl AdapterVersion for Version {
    async fn version(&self) -> Result<String, LakeError> {
        self.0.answer()?;
        Ok(self.0.version.lock().unwrap().clone())
    }
}

struct At(Arc<Adapter>);

#[async_trait]
impl LakeAdmin for At {
    async fn ensure_warehouse(
        &self,
        n: &str,
        r: &RetentionRules,
    ) -> Result<WarehouseMade, LakeError> {
        self.0.lake.ensure_warehouse(n, r).await
    }
    async fn replicate(&self, n: &str, t: &ReplicaTarget) -> Result<ReplicationState, LakeError> {
        self.0.lake.replicate(n, t).await
    }
    async fn replication(&self, n: &str) -> Result<ReplicationSync, LakeError> {
        self.0.lake.replication(n).await
    }
    async fn set_managed_policies(&self, p: &[ManagedPolicy]) -> Result<(), LakeError> {
        self.0.lake.set_managed_policies(p).await
    }
    async fn session_policy(&self, grant: &LakeGrant) -> Result<SessionPolicy, LakeError> {
        self.0.answer()?;
        self.0.asked.fetch_add(1, Ordering::SeqCst);
        self.0.lake.session_policy(grant).await
    }
    async fn ensure_credentials(
        &self,
        k: LakeCredentialKind,
        h: Option<&LakeKeys>,
    ) -> Result<LakeKeys, LakeError> {
        self.0.lake.ensure_credentials(k, h).await
    }
    async fn health(&self) -> Result<LakeHealth, LakeError> {
        self.0.lake.health().await
    }
}

fn policies(ttl: Duration) -> (SessionPolicies, Arc<Adapter>) {
    let adapter = Arc::new(Adapter::default());
    *adapter.version.lock().unwrap() = "lake-admin/1 lake-rustfs 0.1.0".into();
    let cache = SessionPolicies::new(
        Arc::new(Admins(adapter.clone())),
        Arc::new(Version(adapter.clone())),
    )
    .keeping(ttl);
    (cache, adapter)
}

#[tokio::test]
async fn a_grant_is_asked_once_while_its_policy_holds_even_with_the_adapter_down() {
    let (cache, adapter) = policies(Duration::from_secs(3600));
    let grant = LakeGrant::read_only(W);
    let first = cache.policy(&grant).await.unwrap();
    assert_eq!(cache.policy(&grant).await.unwrap(), first);
    assert_eq!(adapter.asked.load(Ordering::SeqCst), 1);
    adapter.down.store(true, Ordering::SeqCst);
    assert_eq!(
        cache.policy(&grant).await.unwrap(),
        first,
        "kept while down"
    );
    let other = LakeGrant::metadata(W);
    let refused = cache.policy(&other).await;
    assert!(
        matches!(refused, Err(LakeError::Unavailable(_))),
        "{refused:?}"
    );
}

#[tokio::test]
async fn an_expired_policy_is_asked_again_and_never_served_while_the_adapter_is_down() {
    let (cache, adapter) = policies(Duration::from_millis(30));
    let grant = LakeGrant::read_only(W);
    cache.policy(&grant).await.unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;
    cache.policy(&grant).await.unwrap();
    assert_eq!(adapter.asked.load(Ordering::SeqCst), 2, "asked again");
    tokio::time::sleep(Duration::from_millis(60)).await;
    adapter.down.store(true, Ordering::SeqCst);
    let refused = cache.policy(&grant).await;
    assert!(
        refused.is_err(),
        "an expired policy is not served: {refused:?}"
    );
}

#[tokio::test]
async fn a_new_adapter_version_asks_every_grant_again() {
    let (cache, adapter) = policies(Duration::from_millis(30));
    let grant = LakeGrant::read_only(W);
    cache.policy(&grant).await.unwrap();
    *adapter.version.lock().unwrap() = "lake-admin/1 lake-rustfs 0.2.0".into();
    tokio::time::sleep(Duration::from_millis(60)).await;
    cache.policy(&grant).await.unwrap();
    let other = LakeGrant::metadata(W);
    cache.policy(&other).await.unwrap();
    assert_eq!(adapter.asked.load(Ordering::SeqCst), 3);
    let bad = cache.policy(&LakeGrant::read_only("Not*A*Bucket")).await;
    assert!(matches!(bad, Err(LakeError::Invalid(_))), "{bad:?}");
}
