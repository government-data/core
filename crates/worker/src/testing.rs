//! What the certificate tests share: a CA made in each test, certificates
//! of it as the installation's CA signs them (`O=qd, OU=worker,
//! CN=<machine>`, P-256, the key in PKCS#8 PEM, as the node agent makes it),
//! and the agent's directory written as the agent writes it.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};

/// A CA of a test.
pub(crate) struct TestCa {
    issuer: Issuer<'static, KeyPair>,
    pub(crate) pem: String,
}

/// A certificate and its key, PEM.
#[derive(Clone)]
pub(crate) struct Leaf {
    pub(crate) cert_pem: String,
    pub(crate) key_pem: String,
    pub(crate) der: Vec<u8>,
}

impl Leaf {
    /// `machine.pem`: the certificate then the key.
    pub(crate) fn bundle(&self) -> String {
        format!("{}\n{}\n", self.cert_pem.trim(), self.key_pem.trim())
    }
}

impl TestCa {
    pub(crate) fn new() -> Self {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = CertificateParams::default();
        // As the installation's CA: no default CN first (O, OU, CN in order).
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

    /// A worker machine's certificate, `O=qd, OU=worker, CN=<name>`.
    pub(crate) fn machine(&self, name: &str) -> Leaf {
        self.leaf(Some("worker"), name)
    }

    /// A certificate of another role, `O=qd, OU=<role>, CN=<name>`.
    pub(crate) fn machine_of(&self, role: &str, name: &str) -> Leaf {
        self.leaf(Some(role), name)
    }

    /// A server's certificate for `localhost`.
    pub(crate) fn server(&self) -> Leaf {
        self.leaf(None, "localhost")
    }

    fn leaf(&self, role: Option<&str>, name: &str) -> Leaf {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = CertificateParams::new(vec![name.to_string()]).unwrap();
        // As the installation's CA: no default CN first (O, OU, CN in order).
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::OrganizationName, "qd");
        if let Some(role) = role {
            params
                .distinguished_name
                .push(DnType::OrganizationalUnitName, role);
        }
        params.distinguished_name.push(DnType::CommonName, name);
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ClientAuth,
            ExtendedKeyUsagePurpose::ServerAuth,
        ];
        params.use_authority_key_identifier_extension = true;
        let cert = params.signed_by(&key, &self.issuer).unwrap();
        Leaf {
            cert_pem: cert.pem(),
            key_pem: key.serialize_pem(),
            der: cert.der().to_vec(),
        }
    }
}

/// A directory of a test, removed when it ends.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("qd-worker-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The agent's files for `leaf` of the CA `ca_pem`, each written aside and
/// renamed, in the agent's order (the bundle, the key, the certificate).
pub(crate) fn write_machine(dir: &Path, ca_pem: &str, leaf: &Leaf) {
    put(dir, "ca.crt", ca_pem, 0o644);
    put(dir, "machine.pem", &leaf.bundle(), 0o600);
    put(dir, "machine.key", &leaf.key_pem, 0o600);
    put(dir, "machine.crt", &leaf.cert_pem, 0o644);
}

fn put(dir: &Path, name: &str, contents: &str, mode: u32) {
    let aside = dir.join(format!(".{name}.new"));
    std::fs::write(&aside, contents).unwrap();
    std::fs::set_permissions(&aside, std::fs::Permissions::from_mode(mode)).unwrap();
    std::fs::rename(&aside, dir.join(name)).unwrap();
}
