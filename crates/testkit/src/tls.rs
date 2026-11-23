//! A test CA as the installation's (decision 0066): machine certificates
//! `O=qd, OU=<role>, CN=<name>` for client and server use, made in memory.
//! Synthetic keys only, never written to disk.

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};

/// A CA of its own for one test.
pub struct TestCa {
    issuer: Issuer<'static, KeyPair>,
    /// Its certificate, PEM.
    pub pem: String,
}

impl std::fmt::Debug for TestCa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TestCa(…)")
    }
}

/// A certificate and its key, PEM, and the certificate's DER.
#[derive(Clone)]
pub struct Leaf {
    pub cert_pem: String,
    pub key_pem: String,
    pub der: Vec<u8>,
}

impl std::fmt::Debug for Leaf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Leaf(…)")
    }
}

impl Leaf {
    /// The certificate then the key, as a machine's `machine.pem`.
    pub fn bundle(&self) -> String {
        format!("{}\n{}\n", self.cert_pem.trim(), self.key_pem.trim())
    }
}

impl Default for TestCa {
    fn default() -> Self {
        Self::new()
    }
}

impl TestCa {
    pub fn new() -> Self {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("a P-256 key");
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
        let cert = params.self_signed(&key).expect("the test CA");
        Self {
            pem: cert.pem(),
            issuer: Issuer::new(params, key),
        }
    }

    /// A machine's certificate of `role`, named `name` (also its DNS name).
    pub fn machine(&self, role: &str, name: &str) -> Leaf {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("a P-256 key");
        let mut params = CertificateParams::new(vec![name.to_string()]).expect("a DNS name");
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::OrganizationName, "qd");
        params
            .distinguished_name
            .push(DnType::OrganizationalUnitName, role);
        params.distinguished_name.push(DnType::CommonName, name);
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ClientAuth,
            ExtendedKeyUsagePurpose::ServerAuth,
        ];
        params.use_authority_key_identifier_extension = true;
        let cert = params.signed_by(&key, &self.issuer).expect("a leaf");
        Leaf {
            cert_pem: cert.pem(),
            key_pem: key.serialize_pem(),
            der: cert.der().to_vec(),
        }
    }
}
