//! The replica's admin keys pasted in Settings → Lake (0066 step 11, the
//! user's ruling of 2026-11-13): kept, replaced when pasted again, never
//! answered back, audited without them.

use auth_gateway::{Lake, LakeAdminKeyStore, LakeAdminKeys, LakeAdminKeysKept};

use super::*;

/// What a person pasted, per lake.
#[derive(Default)]
struct Kept(Mutex<Vec<(String, LakeAdminKeys, LakeAdminKeysKept)>>);

#[async_trait]
impl LakeAdminKeyStore for Kept {
    async fn set(
        &self,
        lake: &Lake,
        keys: &LakeAdminKeys,
        by: &AdminId,
        at: SystemTime,
    ) -> Result<LakeAdminKeysKept, RepositoryError> {
        let kept = LakeAdminKeysKept {
            access_key: keys.access_key.clone(),
            set_by: by.clone(),
            set_at: at,
        };
        let mut all = self.0.lock().unwrap();
        all.retain(|(l, _, _)| *l != lake.key());
        all.push((lake.key(), keys.clone(), kept.clone()));
        Ok(kept)
    }

    async fn kept(&self, lake: &Lake) -> Result<Option<LakeAdminKeysKept>, RepositoryError> {
        let all = self.0.lock().unwrap();
        Ok(all
            .iter()
            .find(|(l, _, _)| *l == lake.key())
            .map(|(_, _, k)| k.clone()))
    }
}

fn with_store(store: Arc<Kept>) -> World {
    let w = world();
    World {
        service: w.service.with_lake_admin_keys(store),
        audit: w.audit,
        clock: w.clock,
    }
}

#[tokio::test]
async fn the_replicas_admin_keys_are_kept_replaced_and_audited_without_them() {
    let store = Arc::new(Kept::default());
    let w = with_store(store.clone());
    let s = &w.service;
    let (by, from) = (admin(), origin());
    assert_eq!(s.lake_admin_keys(&Lake::Replica).await, Ok(None));
    let first = "ab".repeat(32);
    let padded = format!(" {first} ");
    let kept = s.set_lake_admin_keys(&Lake::Replica, " qd-platform-admin\n", &padded, &by, &from);
    let kept = kept.await.unwrap();
    assert_eq!(kept.access_key, "qd-platform-admin");
    assert_eq!(kept.set_by, by);
    assert_eq!(s.lake_admin_keys(&Lake::Replica).await, Ok(Some(kept)));
    let held = store.0.lock().unwrap()[0].clone();
    assert_eq!(
        (held.0.as_str(), held.1.secret_key.as_str()),
        ("replica", first.as_str())
    );
    // Pasted again: replaced, one pair.
    w.advance(Duration::from_secs(60));
    let second = "cd".repeat(32);
    let again = s.set_lake_admin_keys(&Lake::Replica, "qd-platform-admin", &second, &by, &from);
    assert_eq!(
        again.await.unwrap().set_at,
        SystemTime::UNIX_EPOCH + Duration::from_secs(60)
    );
    assert_eq!(store.0.lock().unwrap().len(), 1);
    assert_eq!(store.0.lock().unwrap()[0].1.secret_key, second);
    // Audited each time, by the person, never with the keys.
    let events = w.audit.0.lock().unwrap().clone();
    assert_eq!(events.len(), 2);
    let e = &events[1];
    assert_eq!(e.event_type, AuditEventType::SecretsChanged);
    assert_eq!(
        (e.action.as_str(), e.resource.as_str()),
        ("lake:admin-keys", "lake/replica")
    );
    assert_eq!(
        (e.admin_id.as_ref(), e.ip.as_str()),
        (Some(&by), "192.0.2.10")
    );
    let logged = format!("{events:?} {:?}", store.0.lock().unwrap());
    assert!(!logged.contains(&first) && !logged.contains(&second));
}

#[tokio::test]
async fn keys_out_of_form_or_without_a_store_are_not_kept() {
    let store = Arc::new(Kept::default());
    let w = with_store(store.clone());
    let s = &w.service;
    let (by, from) = (admin(), origin());
    let secret = "ab".repeat(32);
    for access in ["", "qd", "qd platform", "qd\"admin"] {
        let refused = s
            .set_lake_admin_keys(&Lake::Replica, access, &secret, &by, &from)
            .await;
        assert_eq!(refused, Err(LakeAdminError::InvalidAccessKey), "{access:?}");
    }
    let short = "ab".repeat(20);
    let upper = "AB".repeat(32);
    for secret in ["", short.as_str(), upper.as_str(), "secret"] {
        let refused =
            s.set_lake_admin_keys(&Lake::Replica, "qd-platform-admin", secret, &by, &from);
        assert_eq!(refused.await, Err(LakeAdminError::InvalidSecret));
    }
    assert!(store.0.lock().unwrap().is_empty() && w.kinds().is_empty());
    let bare = world();
    let none =
        bare.service
            .set_lake_admin_keys(&Lake::Replica, "qd-platform-admin", &secret, &by, &from);
    assert_eq!(none.await, Err(LakeAdminError::Unavailable));
    assert_eq!(
        bare.service.lake_admin_keys(&Lake::Replica).await,
        Err(LakeAdminError::Unavailable)
    );
    assert!(bare.kinds().is_empty());
}

#[tokio::test]
async fn the_main_lakes_keys_are_kept_apart_from_the_replicas() {
    let store = Arc::new(Kept::default());
    let w = with_store(store.clone());
    let s = &w.service;
    let (by, from) = (admin(), origin());
    let (main, replica) = ("ab".repeat(32), "cd".repeat(32));
    let set = |lake: Lake, secret: String| async move {
        s.set_lake_admin_keys(&lake, "qd-platform-admin", &secret, &admin(), &origin())
            .await
    };
    set(Lake::main(), main.clone()).await.unwrap();
    assert_eq!(s.lake_admin_keys(&Lake::Replica).await, Ok(None));
    set(Lake::Replica, replica.clone()).await.unwrap();
    let held: Vec<(String, String)> = store
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|(l, k, _)| (l.clone(), k.secret_key.clone()))
        .collect();
    assert_eq!(
        held,
        [("pool:main".into(), main), ("replica".into(), replica)]
    );
    let kept = s.lake_admin_keys(&Lake::main()).await.unwrap().unwrap();
    assert_eq!(kept.set_by, by);
    let events = w.audit.0.lock().unwrap().clone();
    let resources: Vec<&str> = events.iter().map(|e| e.resource.as_str()).collect();
    assert_eq!(resources, ["lake/pool:main", "lake/replica"]);
    assert!(events.iter().all(|e| e.ip == from.ip));
}
