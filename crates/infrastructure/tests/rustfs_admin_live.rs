//! The lakes' admin API's adapter against a real RustFS behind the
//! repository's gate: run by `tools/verify/rustfs_replication_gate.sh`,
//! which starts them and gives the gate's port, the CA, the application
//! machine's `machine.pem` (`LAKE_CA_FILE`, `LAKE_CLIENT_CERT_FILE`) and the
//! admin keys' file; ignored otherwise.

use std::sync::Arc;

use async_trait::async_trait;
use auth_gateway::{Lake, LakeAdmin, LakeAdminKeys, LakeProbe};
use domain::RepositoryError;
use infrastructure::rustfs_admin::{AdminKeySource, RustfsLakeAdmin};

struct Keys(LakeAdminKeys);

#[async_trait]
impl AdminKeySource for Keys {
    async fn open(&self, _: &Lake) -> Result<Option<LakeAdminKeys>, RepositoryError> {
        Ok(Some(self.0.clone()))
    }
}

fn var(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name}: run tools/verify/rustfs_replication_gate.sh"))
}

/// The keys `join.sh` would have shown, from the script's file.
fn kept() -> LakeAdminKeys {
    let file = std::fs::read_to_string(var("QD_VERIFY_ADMIN_KEYS")).unwrap();
    let get = |name: &str| {
        let line = file.lines().find(|l| l.starts_with(name)).unwrap();
        line[name.len() + 1..].trim().to_string()
    };
    LakeAdminKeys {
        access_key: get("ADMIN_ACCESS_KEY"),
        secret_key: get("ADMIN_SECRET_KEY"),
    }
}

#[tokio::test]
#[ignore = "needs tools/verify/rustfs_replication_gate.sh"]
async fn a_replica_behind_its_gate_answers_its_drives_to_the_pasted_keys() {
    let port: u16 = var("QD_VERIFY_GATE_PORT").parse().unwrap();
    let node = var("QD_VERIFY_NODE");
    let ask = |keys: LakeAdminKeys| {
        let api = RustfsLakeAdmin::from_env(Arc::new(Keys(keys))).unwrap();
        api.at("https", port)
    };
    let probe = ask(kept()).probe(&Lake::Replica, &node).await.unwrap();
    assert!(
        matches!(
            probe,
            LakeProbe::Answered {
                drives_online: 1,
                drives_offline: 0
            }
        ),
        "{probe:?}"
    );
    let wrong = LakeAdminKeys {
        secret_key: "0".repeat(64),
        ..kept()
    };
    let probe = ask(wrong).probe(&Lake::Replica, &node).await.unwrap();
    let refused = LakeProbe::KeysRefused {
        code: "SignatureDoesNotMatch".into(),
    };
    assert_eq!(probe, refused);
    // Trusting the CA but presenting no certificate: the gate refuses it.
    let ca = std::fs::read(var("LAKE_CA_FILE")).unwrap();
    let bare = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(&ca).unwrap())
        .build()
        .unwrap();
    let api = RustfsLakeAdmin::new(bare, Arc::new(Keys(kept()))).at("https", port);
    let probe = api.probe(&Lake::Replica, &node).await.unwrap();
    assert!(matches!(probe, LakeProbe::Unreachable { .. }), "{probe:?}");
    println!("without a certificate: {probe:?}");
}
