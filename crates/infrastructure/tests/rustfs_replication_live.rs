//! The replication's adapter against two real RustFS, the main lake and
//! the replica, each behind the repository's gate: run by
//! `tools/verify/rustfs_replication_app.sh`, which starts them and gives
//! the addresses, the CA and the application machine's `machine.pem`
//! (`LAKE_CA_FILE`, `LAKE_CLIENT_CERT_FILE`) and both lakes' admin keys'
//! files; ignored otherwise. Run once per configuration run.

use std::sync::Arc;

use async_trait::async_trait;
use auth_gateway::{Lake, LakeAdminKeys, ReplicationOutcome};
use domain::RepositoryError;
use infrastructure::rustfs_admin::{AdminKeySource, RustfsLakeAdmin};
use infrastructure::rustfs_replication::WarehouseReplication;

fn var(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name}: run tools/verify/rustfs_replication_app.sh"))
}

/// The keys the lakes' scripts would have shown, from the script's files.
fn keys(file: &str) -> LakeAdminKeys {
    let file = std::fs::read_to_string(var(file)).unwrap();
    let get = |name: &str| {
        let line = file.lines().find(|l| l.starts_with(name)).unwrap();
        line[name.len() + 1..].trim().to_string()
    };
    LakeAdminKeys {
        access_key: get("ADMIN_ACCESS_KEY"),
        secret_key: get("ADMIN_SECRET_KEY"),
    }
}

struct Kept;

#[async_trait]
impl AdminKeySource for Kept {
    async fn open(&self, lake: &Lake) -> Result<Option<LakeAdminKeys>, RepositoryError> {
        Ok(Some(match lake {
            Lake::Replica => keys("QD_VERIFY_REPLICA_KEYS"),
            _ => keys("QD_VERIFY_MAIN_KEYS"),
        }))
    }
}

#[tokio::test]
#[ignore = "needs tools/verify/rustfs_replication_app.sh"]
async fn the_application_configures_a_warehouses_replication() {
    let port: u16 = var("QD_VERIFY_REPLICA_PORT").parse().unwrap();
    let api = RustfsLakeAdmin::from_env(Arc::new(Kept))
        .unwrap()
        .at("https", port);
    let plan = |warehouse: String| WarehouseReplication {
        warehouse,
        source: var("QD_VERIFY_SOURCE"),
        replica: var("QD_VERIFY_NODE"),
        proxy: var("QD_VERIFY_PROXY"),
    };
    let outcome = api
        .configure(&plan(var("QD_VERIFY_WAREHOUSE")))
        .await
        .unwrap();
    println!("configured: {outcome:?}");
    assert_eq!(outcome, ReplicationOutcome::Configured);
    let missing = plan("qd-tenant-0000000000000000000000000000ffff".into());
    let outcome = api.configure(&missing).await.unwrap();
    println!("a warehouse the main lake does not have: {outcome:?}");
    assert_eq!(outcome, ReplicationOutcome::NoWarehouse);
}
