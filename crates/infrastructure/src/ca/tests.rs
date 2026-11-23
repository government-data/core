//! The CA, with keys made in each test: none is kept in the repository.

use std::path::PathBuf;
use std::time::Duration as StdDuration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::Utc;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose, PublicKeyData,
    SignatureAlgorithm, SigningKey, PKCS_ECDSA_P256_SHA256, PKCS_ECDSA_P384_SHA384,
    PKCS_RSA_SHA256,
};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use sha2::{Digest, Sha256};
use x509_parser::prelude::{FromDer, GeneralName, X509Certificate};

use super::inspect::pem_contents;
use super::*;

fn new_ca(name: &str) -> (NewCa, Ca) {
    let made = Ca::create(name, DEFAULT_CA_VALIDITY).unwrap();
    let ca = Ca::load(&made.cert_pem, &made.key_pem).unwrap();
    (made, ca)
}

/// A request from `key`, asking for nothing in particular.
fn request(key: &impl SigningKey) -> String {
    let mut params = CertificateParams::default();
    params
        .distinguished_name
        .push(DnType::CommonName, "anything");
    params.serialize_request(key).unwrap().pem().unwrap()
}

fn machine() -> MachineIdentity {
    MachineIdentity::new("worker", "worker-1", ["worker-1.qd.internal", "10.0.0.7"]).unwrap()
}

fn der(pem: &str) -> Vec<u8> {
    pem_contents(pem, "CERTIFICATE").unwrap()
}

fn pem(label: &str, der: &[u8]) -> String {
    format!(
        "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
        STANDARD.encode(der)
    )
}

/// Whether `cert_pem` verifies against `ca_pem` as rustls would, for `usage`.
fn verifies(ca_pem: &str, cert_pem: &str, usage: webpki::KeyUsage) -> bool {
    let ca_der = CertificateDer::from(der(ca_pem));
    let anchor = webpki::anchor_from_trusted_cert(&ca_der).unwrap();
    let cert_der = CertificateDer::from(der(cert_pem));
    let cert = webpki::EndEntityCert::try_from(&cert_der).unwrap();
    cert.verify_for_usage(
        webpki::ALL_VERIFICATION_ALGS,
        &[anchor],
        &[],
        UnixTime::now(),
        usage,
        None,
        None,
    )
    .is_ok()
}

#[test]
fn the_ca_is_p256_a_ca_of_path_length_zero_and_signs_only_certificates() {
    let (made, _) = new_ca("Cooperativa Uno");
    let der = der(&made.cert_pem);
    let (_, cert) = X509Certificate::from_der(&der).unwrap();
    let constraints = cert.basic_constraints().unwrap().unwrap();
    assert!(constraints.critical);
    assert!(constraints.value.ca);
    assert_eq!(constraints.value.path_len_constraint, Some(0));
    let usage = cert.key_usage().unwrap().unwrap().value;
    assert!(usage.key_cert_sign() && usage.crl_sign() && !usage.digital_signature());
    let info = certificate_info(&made.cert_pem).unwrap();
    assert_eq!(info.organization.as_deref(), Some("qd"));
    assert_eq!(info.common_name.as_deref(), Some("Cooperativa Uno CA"));
    let years = (info.not_after - Utc::now()).num_days();
    assert!((3640..=3650).contains(&years), "{years}");
    assert!(made.key_pem.contains("BEGIN PRIVATE KEY"));
    assert!(!format!("{made:?}").contains("PRIVATE"));
    let fingerprint = sha256_fingerprint(&made.cert_pem).unwrap();
    assert_eq!(fingerprint, super::inspect::hex(&Sha256::digest(&der)));
    assert_eq!(fingerprint.len(), 64);
    assert!(Ca::create("", DEFAULT_CA_VALIDITY).is_err());
    assert!(Ca::create("a\nb", DEFAULT_CA_VALIDITY).is_err());
}

#[test]
fn a_signed_certificate_verifies_for_client_and_server_and_not_against_another_ca() {
    let (made, ca) = new_ca("uno");
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let issued = ca
        .sign(&request(&key), &machine(), Lifetime::MACHINE)
        .unwrap();
    assert!(verifies(
        &made.cert_pem,
        &issued.pem,
        webpki::KeyUsage::client_auth()
    ));
    assert!(verifies(
        &made.cert_pem,
        &issued.pem,
        webpki::KeyUsage::server_auth()
    ));
    let cert_der = CertificateDer::from(der(&issued.pem));
    let cert = webpki::EndEntityCert::try_from(&cert_der).unwrap();
    for name in ["worker-1.qd.internal", "10.0.0.7"] {
        let name = ServerName::try_from(name).unwrap();
        assert!(cert.verify_is_valid_for_subject_name(&name).is_ok());
    }
    let (other, _) = new_ca("uno");
    assert!(!verifies(
        &other.cert_pem,
        &issued.pem,
        webpki::KeyUsage::client_auth()
    ));
    assert!(!verifies(
        &other.cert_pem,
        &issued.pem,
        webpki::KeyUsage::server_auth()
    ));
}

#[test]
fn the_subject_names_and_uses_are_the_cas_whatever_the_request_asks() {
    let (_, ca) = new_ca("uno");
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut asked = CertificateParams::new(vec!["evil.example".to_string()]).unwrap();
    asked
        .distinguished_name
        .push(DnType::OrganizationName, "qd");
    asked
        .distinguished_name
        .push(DnType::OrganizationalUnitName, "admin");
    asked.distinguished_name.push(DnType::CommonName, "root");
    asked.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    asked.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let csr = asked.serialize_request(&key).unwrap().pem().unwrap();
    let issued = ca.sign(&csr, &machine(), Lifetime::MACHINE).unwrap();

    let info = certificate_info(&issued.pem).unwrap();
    assert_eq!(info.organization.as_deref(), Some("qd"));
    assert_eq!(info.unit.as_deref(), Some("worker"));
    assert_eq!(info.common_name.as_deref(), Some("worker-1"));
    let der = der(&issued.pem);
    let (_, cert) = X509Certificate::from_der(&der).unwrap();
    assert!(!cert.is_ca());
    let usage = cert.key_usage().unwrap().unwrap().value;
    assert!(usage.digital_signature() && !usage.key_cert_sign());
    let eku = cert.extended_key_usage().unwrap().unwrap().value;
    assert!(eku.client_auth && eku.server_auth && !eku.any);
    let names: Vec<String> = cert
        .subject_alternative_name()
        .unwrap()
        .unwrap()
        .value
        .general_names
        .iter()
        .map(|n| match n {
            GeneralName::DNSName(d) => d.to_string(),
            GeneralName::IPAddress(ip) => format!("{ip:?}"),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(names, ["worker-1.qd.internal", "[10, 0, 0, 7]"]);
    // The key is the request's.
    assert_eq!(
        cert.public_key().subject_public_key.data.as_ref(),
        key.public_key_raw()
    );
    // Without names there is no SAN at all.
    let bare = MachineIdentity::new("worker", "w2", Vec::<String>::new()).unwrap();
    let issued = ca.sign(&csr, &bare, Lifetime::MACHINE).unwrap();
    let der = super::inspect::pem_contents(&issued.pem, "CERTIFICATE").unwrap();
    let (_, cert) = X509Certificate::from_der(&der).unwrap();
    assert!(cert.subject_alternative_name().unwrap().is_none());
}

#[test]
fn a_bad_signature_or_a_key_other_than_p256_is_refused() {
    let (_, ca) = new_ca("uno");
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut csr = pem_contents(&request(&key), "CERTIFICATE REQUEST").unwrap();
    let last = csr.len() - 1;
    csr[last] ^= 0x01;
    let refused = ca.sign(
        &pem("CERTIFICATE REQUEST", &csr),
        &machine(),
        Lifetime::MACHINE,
    );
    assert!(
        matches!(&refused, Err(CaError::Request(m)) if m.contains("signature")),
        "{refused:?}"
    );

    let p384 = KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384).unwrap();
    let refused = ca.sign(&request(&p384), &machine(), Lifetime::MACHINE);
    assert!(matches!(&refused, Err(CaError::Request(m)) if m.contains("P-256")));

    let refused = ca.sign(&request(&RsaKey::new()), &machine(), Lifetime::MACHINE);
    assert!(matches!(&refused, Err(CaError::Request(m)) if m.contains("P-256")));

    for garbage in ["", "not a request", &key.serialize_pem()] {
        assert!(matches!(
            ca.sign(garbage, &machine(), Lifetime::MACHINE),
            Err(CaError::Request(_))
        ));
    }
}

#[test]
fn a_bad_role_name_or_host_name_is_refused() {
    let long = "a".repeat(64);
    for (role, name) in [
        ("Admin", "w1"),
        ("", "w1"),
        ("worker", ""),
        ("worker", "w_1"),
        ("worker", "w.1"),
        ("worker", long.as_str()),
        ("worker", "wörker"),
    ] {
        assert!(
            MachineIdentity::new(role, name, Vec::<String>::new()).is_err(),
            "{role} {name}"
        );
    }
    assert!(MachineIdentity::new("worker", &"a".repeat(63), Vec::<String>::new()).is_ok());
    for host in ["*.qd.internal", "bad host", "-a.b", "a..b", "", "a.b-"] {
        assert!(
            MachineIdentity::new("worker", "w1", [host]).is_err(),
            "{host}"
        );
    }
    assert!(MachineIdentity::new("dolt", "dolt", ["Dolt.QD", "::1"]).is_ok());
}

#[test]
fn the_lifetime_is_the_one_asked_and_never_more_than_a_year() {
    let (_, ca) = new_ca("uno");
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let issued = ca
        .sign(&request(&key), &machine(), Lifetime::MACHINE)
        .unwrap();
    let info = certificate_info(&issued.pem).unwrap();
    let expected = Utc::now() + chrono::Duration::hours(24);
    assert!((info.not_after - expected).num_seconds().abs() < 60);
    assert_eq!(info.not_after, issued.not_after);
    let skew = Utc::now() - info.not_before;
    assert!((290..=310).contains(&skew.num_seconds()), "{skew}");

    let year = StdDuration::from_secs(365 * 24 * 3600);
    assert_eq!(Lifetime::new(year).unwrap(), Lifetime::CEILING);
    assert!(Lifetime::new(year + StdDuration::from_secs(1)).is_err());
    assert!(Lifetime::new(StdDuration::ZERO).is_err());
    let issued = ca
        .sign(&request(&key), &machine(), Lifetime::CEILING)
        .unwrap();
    let days = (certificate_info(&issued.pem).unwrap().not_after - Utc::now()).num_days();
    assert!((364..=365).contains(&days), "{days}");
}

#[test]
fn every_serial_is_new_and_reads_back_as_returned() {
    let (_, ca) = new_ca("uno");
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let csr = request(&key);
    let a = ca.sign(&csr, &machine(), Lifetime::MACHINE).unwrap();
    let b = ca.sign(&csr, &machine(), Lifetime::MACHINE).unwrap();
    assert_ne!(a.serial, b.serial);
    for issued in [&a, &b] {
        assert_eq!(issued.serial.len(), 32);
        assert_eq!(certificate_info(&issued.pem).unwrap().serial, issued.serial);
    }
}

#[test]
fn the_reader_takes_one_certificate_and_nothing_after_it() {
    use auth_gateway::CertificateReader;
    let (_, ca) = new_ca("uno");
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let issued = ca
        .sign(&request(&key), &machine(), Lifetime::MACHINE)
        .unwrap();
    let der = der(&issued.pem);
    let read = X509CertificateReader.read(&der).unwrap();
    assert_eq!(
        (read.organization.as_deref(), read.unit.as_deref()),
        (Some("qd"), Some("worker"))
    );
    assert_eq!(read.common_name.as_deref(), Some("worker-1"));
    assert_eq!(read.serial, issued.serial);
    assert_eq!(
        read.not_after,
        std::time::SystemTime::from(issued.not_after)
    );
    let mut trailing = der.clone();
    trailing.push(0);
    assert_eq!(X509CertificateReader.read(&trailing), None);
    assert_eq!(X509CertificateReader.read(&der[..der.len() - 1]), None);
    assert_eq!(X509CertificateReader.read(b"garbage"), None);
}

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("qd-ca-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_ca_written_and_loaded_again_signs_the_same_and_is_never_overwritten() {
    let dir = scratch();
    let (cert_path, key_path) = (dir.join("ca.crt"), dir.join("ca.key"));
    let made = Ca::create("uno", DEFAULT_CA_VALIDITY).unwrap();
    write_new(&made, &cert_path, &key_path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &PathBuf| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&cert_path), 0o644);
        assert_eq!(mode(&key_path), 0o600);
    }
    let cert_pem = std::fs::read_to_string(&cert_path).unwrap();
    let ca = Ca::load(&cert_pem, &std::fs::read_to_string(&key_path).unwrap()).unwrap();
    assert_eq!(ca.cert_pem(), made.cert_pem);
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let issued = ca
        .sign(&request(&key), &machine(), Lifetime::MACHINE)
        .unwrap();
    assert!(verifies(
        &made.cert_pem,
        &issued.pem,
        webpki::KeyUsage::client_auth()
    ));

    // A second CA is refused on either file, and the first stays as it was.
    let second = Ca::create("dos", DEFAULT_CA_VALIDITY).unwrap();
    let refused = write_new(&second, &cert_path, &dir.join("other.key"));
    assert!(matches!(refused, Err(CaError::File(_))));
    assert!(!dir.join("other.key").exists());
    assert!(write_new(&second, &dir.join("other.crt"), &key_path).is_err());
    assert!(!dir.join("other.crt").exists());
    assert_eq!(std::fs::read_to_string(&cert_path).unwrap(), made.cert_pem);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_key_that_is_not_the_certificates_is_refused() {
    let (one, _) = new_ca("uno");
    let (two, _) = new_ca("dos");
    let refused = Ca::load(&one.cert_pem, &two.key_pem);
    assert!(matches!(refused, Err(CaError::Load(m)) if m.contains("not the certificate's")));
    assert!(Ca::load(&one.key_pem, &one.key_pem).is_err());
    let p384 = KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384).unwrap();
    assert!(Ca::load(&one.cert_pem, &p384.serialize_pem()).is_err());
    // A machine's certificate is not a CA.
    let ca = Ca::load(&one.cert_pem, &one.key_pem).unwrap();
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let issued = ca
        .sign(&request(&key), &machine(), Lifetime::MACHINE)
        .unwrap();
    assert!(Ca::load(&issued.pem, &key.serialize_pem()).is_err());
}

/// An RSA key that signs requests, which ring cannot make.
struct RsaKey {
    private: rsa::RsaPrivateKey,
    public_pkcs1: Vec<u8>,
}

impl RsaKey {
    fn new() -> Self {
        use rsa::pkcs1::EncodeRsaPublicKey;
        let private = rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 1024).unwrap();
        let public_pkcs1 = private.to_public_key().to_pkcs1_der().unwrap().into_vec();
        Self {
            private,
            public_pkcs1,
        }
    }
}

impl PublicKeyData for RsaKey {
    fn der_bytes(&self) -> &[u8] {
        &self.public_pkcs1
    }

    fn algorithm(&self) -> &'static SignatureAlgorithm {
        &PKCS_RSA_SHA256
    }
}

impl SigningKey for RsaKey {
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, rcgen::Error> {
        // PKCS#1 v1.5: the DigestInfo of SHA-256, then the digest.
        let mut digest_info = vec![
            0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
            0x01, 0x05, 0x00, 0x04, 0x20,
        ];
        digest_info.extend_from_slice(&Sha256::digest(msg));
        self.private
            .sign(rsa::Pkcs1v15Sign::new_unprefixed(), &digest_info)
            .map_err(|_| rcgen::Error::RingUnspecified)
    }
}
