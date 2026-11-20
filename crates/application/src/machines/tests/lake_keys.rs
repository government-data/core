//! A lake's root keys, given to a node that is installing (0066 step 11,
//! ruling 1): made once per lake, given for a usable join token of its role
//! as the compose file is, audited every time without them.

use auth_gateway::{Lake, LakeRootKeyStore, LakeRootKeys};

use super::install_files::installing;
use super::*;

/// One pair per lake, made the first time; how many times they were made.
#[derive(Default)]
struct Keys(Mutex<Vec<(String, LakeRootKeys)>>);

#[async_trait]
impl LakeRootKeyStore for Keys {
    async fn keys(&self, lake: &Lake) -> Result<LakeRootKeys, RepositoryError> {
        let mut kept = self.0.lock().unwrap();
        if let Some((_, keys)) = kept.iter().find(|(l, _)| *l == lake.key()) {
            return Ok(keys.clone());
        }
        let n = kept.len();
        let keys = LakeRootKeys {
            access_key: format!("qdaccess{n}"),
            secret_key: format!("secret{n}{}", "s".repeat(30)),
        };
        kept.push((lake.key(), keys.clone()));
        Ok(keys)
    }
}

fn with_keys(keys: Arc<Keys>) -> World {
    let w = installing("https://qd.example", true);
    World {
        service: w.service.with_lake_keys(keys),
        audit: w.audit,
        clock: w.clock,
    }
}

#[tokio::test]
async fn the_replicas_keys_are_given_the_same_for_its_token_and_audited() {
    let keys = Arc::new(Keys::default());
    let w = with_keys(keys.clone());
    let s = &w.service;
    let (by, from) = (admin(), origin());
    let issued = s.issue_join_token(MachineRole::LakeReplica, None, &by, &from);
    let issued = issued.await.unwrap();
    let expected = format!(
        "RUSTFS_ACCESS_KEY=qdaccess0\nRUSTFS_SECRET_KEY=secret0{}\n",
        "s".repeat(30)
    );
    for _ in 0..2 {
        let given = s.role_root_keys(&issued.token, MachineRole::LakeReplica, &from);
        assert_eq!(given.await.unwrap(), expected);
    }
    assert_eq!(keys.0.lock().unwrap().len(), 1, "made once");
    // Audited each time, before they were given, without them.
    let events = w.audit.0.lock().unwrap().clone();
    let given: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == AuditEventType::CredentialsIssued)
        .collect();
    assert_eq!(given.len(), 2);
    assert_eq!(given[0].action, "install:lake-replica");
    assert_eq!(given[0].resource, "lake/replica");
    assert_eq!(given[0].ip, "192.0.2.10");
    let logged = format!("{events:?}");
    assert!(!logged.contains("secret0") && !logged.contains("qdaccess0"));
    // Once the machine joined with the token, refused.
    s.join(&issued.token, "replica-1", "csr", &from)
        .await
        .unwrap();
    let used = s.role_root_keys(&issued.token, MachineRole::LakeReplica, &from);
    assert_eq!(used.await, Err(InstallFileError::Refused));
}

#[tokio::test]
async fn no_keys_without_a_usable_token_of_a_lake_role() {
    let keys = Arc::new(Keys::default());
    let w = with_keys(keys.clone());
    let s = &w.service;
    let (by, from) = (admin(), origin());
    let refused = Err(InstallFileError::Refused);
    let replica = MachineRole::LakeReplica;
    let worker = w.issue().await;
    let as_replica = s.role_root_keys(&worker.token, replica, &from).await;
    assert_eq!(as_replica, refused, "a worker's token");
    let own = s.role_root_keys(&worker.token, Worker, &from).await;
    assert_eq!(own, Err(InstallFileError::NotFound), "a worker has none");
    let unknown = format!("qdj_{:0>43}", 9);
    for token in ["", "qdj_short", unknown.as_str()] {
        assert_eq!(s.role_root_keys(token, replica, &from).await, refused);
    }
    let late = s.issue_join_token(replica, None, &by, &from).await.unwrap();
    w.advance(JOIN_TOKEN_TTL);
    let expired = s.role_root_keys(&late.token, replica, &from).await;
    assert_eq!(expired, refused);
    // A pool node's token names its node (tests/lake_pools.rs).
    let lake = s.issue_join_token(MachineRole::Lake, None, &by, &from);
    assert_eq!(lake.await, Err(MachinesError::Invalid("node")));
    assert!(keys.0.lock().unwrap().is_empty(), "nothing made");
    // Without the store, or without installation files, nothing.
    let bare = installing("https://qd.example", true);
    let issued = bare.service.issue_join_token(replica, None, &by, &from);
    let issued = issued.await.unwrap();
    let none = bare.service.role_root_keys(&issued.token, replica, &from);
    assert_eq!(none.await, Err(InstallFileError::Unavailable));
    let none = world();
    let none = none.service.role_root_keys(&issued.token, replica, &from);
    assert_eq!(none.await, Err(InstallFileError::Unavailable));
    let audited = w.kinds().contains(&AuditEventType::CredentialsIssued);
    assert!(!audited, "nothing given, nothing audited as given");
}
