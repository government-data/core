//! The catalog on a Dolt with the machines' certificates (decision 0066,
//! point 11): TLS only, a client certificate of the CA required, as
//! `infra/dolt/tls/config.yaml` has it. The API connects presenting the
//! application VM's `machine.pem` (certificate and key in one file) and
//! verifying Dolt's name; a renewed file is presented by the next
//! connection. Dolt's restart when free: the hold is taken only when Dolt
//! runs nothing, the catalog's operations wait while it is held and go on
//! once released. Synthetic rows only. Needs Docker.

use std::path::{Path, PathBuf};
use std::time::Duration;

use infrastructure::catalog_dolt::{DatasetImport, DoltCatalog};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};
use sqlx::{Connection, MySqlConnection};

const MAPPING: &str =
    "dataset: loans\nsource: loans\nkey: [id]\nfields:\n  id: { column: ID, type: text }\n";
const CONFIG: &str = "log_level: info
listener:
  host: 0.0.0.0
  port: 3306
  tls_cert: /tls/server.pem
  tls_key: /tls/server.pem
  require_secure_transport: true
  ca_cert: /tls/ca.crt
  require_client_cert: true
data_dir: /var/lib/dolt
";

struct Ca {
    issuer: Issuer<'static, KeyPair>,
    pem: String,
}

impl Ca {
    fn new() -> Self {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::OrganizationName, "qd");
        params
            .distinguished_name
            .push(DnType::CommonName, "qd test CA");
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let cert = params.self_signed(&key).unwrap();
        Self {
            pem: cert.pem(),
            issuer: Issuer::new(params, key),
        }
    }

    /// `O=qd, OU=app, CN=localhost`, the certificate then its key, as the
    /// agent's `machine.pem`.
    fn machine_pem(&self) -> String {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::OrganizationName, "qd");
        params
            .distinguished_name
            .push(DnType::OrganizationalUnitName, "app");
        params
            .distinguished_name
            .push(DnType::CommonName, "localhost");
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ClientAuth,
            ExtendedKeyUsagePurpose::ServerAuth,
        ];
        let cert = params.signed_by(&key, &self.issuer).unwrap();
        format!("{}\n{}\n", cert.pem().trim(), key.serialize_pem().trim())
    }
}

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Written aside and renamed, as the agent does.
fn put(dir: &Path, name: &str, contents: &str) {
    let aside = dir.join(format!(".{name}.new"));
    std::fs::write(&aside, contents).unwrap();
    std::fs::rename(&aside, dir.join(name)).unwrap();
}

#[tokio::test]
async fn the_catalog_on_dolt_with_certificates_and_its_restart_when_free() {
    let ca = Ca::new();
    let dir = Dir(std::env::temp_dir().join(format!("qd-dolt-cert-{}", std::process::id())));
    std::fs::create_dir_all(&dir.0).unwrap();
    put(&dir.0, "ca.crt", &ca.pem);
    put(&dir.0, "machine.pem", &ca.machine_pem());
    let (_dolt, port) = testkit::spawn_dolt_with(vec![
        ("/etc/dolt/servercfg.d/config.yaml".into(), CONFIG.into()),
        ("/tls/server.pem".into(), ca.machine_pem().into_bytes()),
        ("/tls/ca.crt".into(), ca.pem.clone().into_bytes()),
    ])
    .await;
    let (machine, ca_file) = (dir.0.join("machine.pem"), dir.0.join("ca.crt"));
    let tls = format!(
        "ssl-mode=VERIFY_IDENTITY&ssl-ca={}&ssl-cert={m}&ssl-key={m}",
        ca_file.display(),
        m = machine.display()
    );
    let base = format!("mysql://root:root@localhost:{port}?{tls}");

    // Without a client certificate, or without TLS: refused.
    let bare = format!("mysql://root:root@localhost:{port}");
    let no_cert = format!(
        "{bare}?ssl-mode=VERIFY_IDENTITY&ssl-ca={}",
        ca_file.display()
    );
    assert!(
        MySqlConnection::connect(&no_cert).await.is_err(),
        "no certificate"
    );
    let plain = format!("{bare}?ssl-mode=DISABLED");
    assert!(MySqlConnection::connect(&plain).await.is_err(), "no TLS");
    // By an address that is not its name: refused.
    let by_ip = format!("mysql://root:root@127.0.0.1:{port}?{tls}");
    assert!(
        MySqlConnection::connect(&by_ip).await.is_err(),
        "the name is verified"
    );

    let mut root = MySqlConnection::connect(&base)
        .await
        .expect("machine.pem, one file");
    sqlx::query("CREATE DATABASE tenant_a")
        .execute(&mut root)
        .await
        .unwrap();
    let catalog = DoltCatalog::new(&base);
    let import = DatasetImport {
        dataset: "loans",
        description: None,
        mapping: MAPPING,
        rules: &[],
    };
    let author = "Test <test@example.org>";
    catalog.import("tenant_a", &import, author).await.unwrap();

    // Another connection running a statement: not free, nothing held.
    let sleeper = tokio::spawn(async move {
        sqlx::query("SELECT SLEEP(3)")
            .execute(&mut root)
            .await
            .unwrap();
        root
    });
    tokio::time::sleep(Duration::from_millis(700)).await;
    let busy = catalog.hold(Duration::from_secs(300)).await;
    assert_eq!(
        (busy.free, busy.in_flight, busy.running),
        (false, 0, Some(1))
    );
    assert!(catalog.mappings("tenant_a").await.is_ok(), "not held");
    sleeper.await.unwrap().close().await.unwrap();

    // Free: held; an operation waits for the release, then goes on, on a
    // pool made again, presenting the renewed machine.pem.
    let held = catalog.hold(Duration::from_secs(300)).await;
    assert_eq!(
        (held.free, held.in_flight, held.running),
        (true, 0, Some(0))
    );
    put(&dir.0, "machine.pem", &ca.machine_pem());
    let catalog = std::sync::Arc::new(catalog);
    let waiting = {
        let catalog = std::sync::Arc::clone(&catalog);
        tokio::spawn(async move { catalog.mappings("tenant_a").await })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!waiting.is_finished(), "held: the operation waits");
    assert!(catalog.release());
    let mappings = waiting.await.unwrap().expect("after the release");
    assert_eq!(mappings, vec![("loans".to_string(), MAPPING.to_string())]);
    assert!(!catalog.release(), "nothing in force any more");
}
