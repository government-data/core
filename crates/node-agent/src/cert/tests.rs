use std::time::{Duration, SystemTime};

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};

use super::*;

/// A CA made in the test, signing for the keys the agent makes.
pub(crate) struct TestCa {
    issuer: Issuer<'static, KeyPair>,
    pub(crate) cert_pem: String,
}

impl TestCa {
    pub(crate) fn new() -> Self {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = CertificateParams::default();
        params
            .distinguished_name
            .push(DnType::CommonName, "test CA");
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        let cert_pem = params.self_signed(&key).unwrap().pem();
        Self {
            issuer: Issuer::new(params, key),
            cert_pem,
        }
    }

    /// A certificate for `request`'s key, from `from` for `lifetime`.
    pub(crate) fn sign(&self, request: &Request, from: SystemTime, lifetime: Duration) -> String {
        let mut params = CertificateParams::new(vec!["node-01".to_string()]).unwrap();
        params.not_before = from.into();
        params.not_after = (from + lifetime).into();
        params.signed_by(&request.key, &self.issuer).unwrap().pem()
    }

    /// A certificate for the key of a signing request, as the CA does.
    pub(crate) fn sign_csr(&self, csr_pem: &str, from: SystemTime, lifetime: Duration) -> String {
        let csr = rcgen::CertificateSigningRequestParams::from_pem(csr_pem).unwrap();
        let mut params = CertificateParams::new(vec!["node-01".to_string()]).unwrap();
        params.not_before = from.into();
        params.not_after = (from + lifetime).into();
        params
            .signed_by(&csr.public_key, &self.issuer)
            .unwrap()
            .pem()
    }

    /// Credentials of a new key, from `from` for `lifetime`.
    pub(crate) fn credentials(&self, from: SystemTime, lifetime: Duration) -> Credentials {
        let request = new_request("node-01").unwrap();
        request.credentials(&self.sign(&request, from, lifetime))
    }
}

const DAY: Duration = Duration::from_secs(24 * 3600);

fn whole_seconds(t: SystemTime) -> SystemTime {
    let s = t.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    std::time::UNIX_EPOCH + Duration::from_secs(s)
}

#[test]
fn a_request_is_of_a_new_p256_key_and_names_the_machine() {
    let a = new_request("worker-01").unwrap();
    let b = new_request("worker-01").unwrap();
    assert_ne!(a.key.serialize_pem(), b.key.serialize_pem());
    assert!(a.csr_pem.contains("BEGIN CERTIFICATE REQUEST"));
    let parsed = rcgen::CertificateSigningRequestParams::from_pem(&a.csr_pem).unwrap();
    assert_eq!(parsed.public_key.der_bytes(), a.key.der_bytes());
    assert_eq!(a.key.algorithm(), &PKCS_ECDSA_P256_SHA256);
}

#[test]
fn the_validity_is_read_back_and_the_certificate_must_be_of_the_key() {
    let ca = TestCa::new();
    let from = whole_seconds(SystemTime::now());
    let creds = ca.credentials(from, DAY);
    let v = creds.validity().unwrap();
    assert_eq!((v.not_before, v.not_after), (from, from + DAY));
    assert_eq!(v.lifetime(), DAY);
    assert_eq!(v.remaining(from + DAY / 4), DAY * 3 / 4);
    assert_eq!(v.age(from + DAY / 4), DAY / 4);
    assert!(!v.ended(from + DAY / 2) && v.ended(from + DAY));

    let other = new_request("node-01").unwrap();
    let mismatched = Credentials {
        key_pem: other.key.serialize_pem(),
        cert_pem: creds.cert_pem.clone(),
    };
    assert!(mismatched.validity().is_err());
}

#[test]
fn a_bundle_is_the_certificate_then_the_key_and_reads_back() {
    let creds = TestCa::new().credentials(SystemTime::now(), DAY);
    let bundle = creds.bundle();
    let cert_at = bundle.find("BEGIN CERTIFICATE").unwrap();
    assert!(cert_at < bundle.find("BEGIN PRIVATE KEY").unwrap());
    assert_eq!(Credentials::from_bundle(&bundle).unwrap(), creds);
    let reversed = format!("{}{}", creds.key_pem, creds.cert_pem);
    assert_eq!(Credentials::from_bundle(&reversed).unwrap(), creds);
    assert!(Credentials::from_bundle(&creds.cert_pem).is_err());
    assert!(!format!("{creds:?}").contains("BEGIN"), "never printed");
}

#[test]
fn the_ca_is_checked_by_its_sha256_in_any_spelling() {
    let ca = TestCa::new();
    let f = fingerprint(&ca.cert_pem).unwrap();
    assert_eq!(f.len(), 64);
    assert!(is_ca(&ca.cert_pem, &f));
    assert!(is_ca(&ca.cert_pem, &f.to_ascii_uppercase()));
    let colons: Vec<String> = f
        .as_bytes()
        .chunks(2)
        .map(|c| String::from_utf8(c.to_vec()).unwrap())
        .collect();
    assert!(is_ca(&ca.cert_pem, &colons.join(":")));
    assert!(!is_ca(&TestCa::new().cert_pem, &f), "another CA");
    assert!(!is_ca(&ca.cert_pem, ""));
    assert!(!is_ca("garbage", &f));
}
