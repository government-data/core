//! The cases over the two ports, `Lake` and `LakeAdmin`: every step run
//! twice changes nothing, and what a lake answers is what the port says.

use std::time::{Duration, SystemTime};

use application::lake::{
    Lake, LakeAdmin, LakeCredentialKind, LakeError, LakeGrant, ManagedAccess, ManagedPolicy,
    ReplicaTarget, RetentionRules, WriteOnce,
};

/// The provisioning: a warehouse, its replication, the managed policies, a
/// grant's session policy, the lake's own keys and its health. `warehouse`
/// is made by the case.
pub async fn lake_admin(admin: &dyn LakeAdmin, warehouse: &str) {
    let rules = RetentionRules {
        versioning: true,
        locked_days: None,
        noncurrent_expire_days: Some(7),
    };
    for _ in 0..2 {
        admin
            .ensure_warehouse(warehouse, &rules)
            .await
            .expect("a warehouse made unless there");
    }
    let replica = ReplicaTarget {
        endpoint: "https://replica.lake.test".into(),
    };
    let first = admin
        .replicate(warehouse, &replica)
        .await
        .expect("replicated");
    let again = admin
        .replicate(warehouse, &replica)
        .await
        .expect("replicated again");
    assert_eq!(first, again, "replicating twice answers the same");
    assert_eq!(
        admin.replication(warehouse).await.expect("its state"),
        again
    );

    let policies = [
        ManagedPolicy {
            name: "quality-viewer".into(),
            access: ManagedAccess::ReadUnder("reports/".into()),
        },
        ManagedPolicy {
            name: "quality-admin".into(),
            access: ManagedAccess::Administer,
        },
    ];
    for _ in 0..2 {
        admin
            .set_managed_policies(&policies)
            .await
            .expect("the managed policies set to exactly these");
    }

    let grant = LakeGrant::read_only(warehouse);
    let policy = admin
        .session_policy(&grant)
        .await
        .expect("a session policy");
    let again = admin.session_policy(&grant).await.expect("again");
    assert_eq!(policy, again, "a grant is translated the same twice");
    let bad = admin
        .session_policy(&LakeGrant::read_only("Not*A*Bucket"))
        .await;
    assert!(matches!(bad, Err(LakeError::Invalid(_))), "{bad:?}");

    for kind in [LakeCredentialKind::Admin, LakeCredentialKind::StsIssuer] {
        let made = admin
            .ensure_credentials(kind, None)
            .await
            .expect("keys made");
        let kept = admin
            .ensure_credentials(kind, Some(&made))
            .await
            .expect("kept");
        assert_eq!(kept, made, "the keys the core holds are kept while taken");
    }
    admin.health().await.expect("its health");
}

/// The data: namespaces, tables, an object written once and a job's
/// credentials. `warehouse` must be made already.
pub async fn lake(lake: &dyn Lake, warehouse: &str) {
    let namespace = "contract_bronze";
    for _ in 0..2 {
        lake.ensure_namespace(warehouse, namespace)
            .await
            .expect("a namespace made unless there");
    }
    let tables = lake.tables(warehouse, namespace).await.expect("its tables");
    assert!(
        tables.is_empty(),
        "a new namespace has no table: {tables:?}"
    );
    let missing = lake.table_location(warehouse, namespace, "nothing").await;
    assert_eq!(missing.expect("asked"), None);
    let dropped = lake.drop_table(warehouse, namespace, "nothing").await;
    assert!(!dropped.expect("asked"), "no such table, nothing dropped");

    let key = "manifests/contract-once.json";
    let first = lake
        .write_once(warehouse, key, b"{\"n\":1}")
        .await
        .expect("written");
    let second = lake
        .write_once(warehouse, key, b"{\"n\":2}")
        .await
        .expect("asked");
    assert_eq!(
        (first, second),
        (WriteOnce::Written, WriteOnce::AlreadyThere)
    );

    let minimum = lake.properties().sts_min_duration;
    let issued = lake
        .credentials(
            &LakeGrant::read_only(warehouse),
            "contract",
            Duration::from_secs(60),
        )
        .await
        .expect("a job's credentials");
    let at_least = SystemTime::now() + minimum - Duration::from_secs(30);
    assert!(
        issued.expires_at >= at_least,
        "never shorter than the lake gives"
    );
    let bad = lake
        .credentials(&LakeGrant::read_only("Not*A*Bucket"), "c", minimum)
        .await;
    assert!(matches!(bad, Err(LakeError::Invalid(_))), "{bad:?}");
}
