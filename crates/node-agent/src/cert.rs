//! The machine's key and certificate: made here, the key never leaves the
//! machine (0066, point 2), and read back for their dates, their key and
//! the CA's fingerprint. Nothing here is logged.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rcgen::{CertificateParams, DnType, KeyPair, PublicKeyData, PKCS_ECDSA_P256_SHA256};
use sha2::{Digest, Sha256};
use x509_parser::prelude::{FromDer, X509Certificate};

const CERT_LABEL: &str = "CERTIFICATE";
const KEY_LABEL: &str = "PRIVATE KEY";

/// A key and the certificate the CA signed for it, both PEM.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub key_pem: String,
    pub cert_pem: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials").finish_non_exhaustive()
    }
}

impl Credentials {
    /// One PEM file, the certificate then the key: what the agent presents.
    pub fn bundle(&self) -> String {
        format!("{}\n{}\n", self.cert_pem.trim(), self.key_pem.trim())
    }

    /// The certificate and the key of a [`Self::bundle`], in either order.
    pub fn from_bundle(pem: &str) -> Result<Self, String> {
        let cert_pem = block(pem, CERT_LABEL).ok_or("no certificate in the bundle")?;
        let key_pem = block(pem, KEY_LABEL).ok_or("no key in the bundle")?;
        Ok(Self { key_pem, cert_pem })
    }

    /// The certificate's dates, once it is checked to be of this key.
    pub fn validity(&self) -> Result<Validity, String> {
        let key = KeyPair::from_pem(&self.key_pem).map_err(|_| "the key cannot be read")?;
        let der = der(&self.cert_pem)?;
        let cert = parse(&der)?;
        if cert.public_key().subject_public_key.data.as_ref() != key.der_bytes() {
            return Err("the certificate is not of this key".into());
        }
        Ok(Validity::of(&cert))
    }
}

/// A new key, and its signing request asking for `name`. The CA keeps only
/// the request's key; the subject is put there anyway.
#[derive(Debug)]
pub struct Request {
    pub key: KeyPair,
    pub csr_pem: String,
}

pub fn new_request(name: &str) -> Result<Request, String> {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(|e| e.to_string())?;
    let mut params = CertificateParams::new(vec![name.to_string()]).map_err(|e| e.to_string())?;
    params.distinguished_name.push(DnType::CommonName, name);
    let csr = params.serialize_request(&key).map_err(|e| e.to_string())?;
    let csr_pem = csr.pem().map_err(|e| e.to_string())?;
    Ok(Request { key, csr_pem })
}

impl Request {
    /// The key with the certificate answered for it.
    pub fn credentials(&self, cert_pem: &str) -> Credentials {
        Credentials {
            key_pem: self.key.serialize_pem(),
            cert_pem: cert_pem.trim().to_string() + "\n",
        }
    }
}

/// When a certificate starts and ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Validity {
    pub not_before: SystemTime,
    pub not_after: SystemTime,
}

impl Validity {
    fn of(cert: &X509Certificate<'_>) -> Self {
        let at = |t: i64| UNIX_EPOCH + Duration::from_secs(u64::try_from(t).unwrap_or(0));
        Self {
            not_before: at(cert.validity().not_before.timestamp()),
            not_after: at(cert.validity().not_after.timestamp()),
        }
    }

    pub fn lifetime(&self) -> Duration {
        self.not_after
            .duration_since(self.not_before)
            .unwrap_or_default()
    }

    /// What is left of it at `now`; zero once it ended.
    pub fn remaining(&self, now: SystemTime) -> Duration {
        self.not_after.duration_since(now).unwrap_or_default()
    }

    /// How long it has been valid at `now`.
    pub fn age(&self, now: SystemTime) -> Duration {
        now.duration_since(self.not_before).unwrap_or_default()
    }

    pub fn ended(&self, now: SystemTime) -> bool {
        now >= self.not_after
    }
}

/// The SHA-256 of a certificate's DER, lowercase hex: what `server ca-init`
/// prints and the join command passes as `--ca-sha256`.
pub fn fingerprint(cert_pem: &str) -> Result<String, String> {
    let digest = Sha256::digest(der(cert_pem)?);
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// Whether `cert_pem` is the CA of fingerprint `expected`, written in hex
/// in either case, with or without colons.
pub fn is_ca(cert_pem: &str, expected: &str) -> bool {
    let expected: String = expected
        .chars()
        .filter(|c| *c != ':' && !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    fingerprint(cert_pem).is_ok_and(|f| !expected.is_empty() && f == expected)
}

fn der(cert_pem: &str) -> Result<Vec<u8>, String> {
    let (_, pem) = x509_parser::pem::parse_x509_pem(cert_pem.trim().as_bytes())
        .map_err(|_| "the certificate is not PEM")?;
    if pem.label != CERT_LABEL {
        return Err("a PEM certificate was expected".into());
    }
    Ok(pem.contents)
}

fn parse(der: &[u8]) -> Result<X509Certificate<'_>, String> {
    let (_, cert) =
        X509Certificate::from_der(der).map_err(|_| "the certificate cannot be parsed")?;
    Ok(cert)
}

/// The first PEM block labelled `label`, with its lines.
fn block(pem: &str, label: &str) -> Option<String> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let start = pem.find(&begin)?;
    let stop = pem[start..].find(&end)? + start + end.len();
    Some(pem[start..stop].to_string() + "\n")
}

#[cfg(test)]
pub(crate) mod tests;
