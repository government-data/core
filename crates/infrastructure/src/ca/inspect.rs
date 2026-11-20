//! Reading a certificate back: its subject, serial and expiry, and the
//! fingerprint the join command checks the CA with (0029's `--ca-sha256`).

use std::time::SystemTime;

use auth_gateway::{CertificateReader, MachineCertificate};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use x509_parser::der_parser::Oid;
use x509_parser::oid_registry::{
    OID_X509_COMMON_NAME, OID_X509_ORGANIZATIONAL_UNIT, OID_X509_ORGANIZATION_NAME,
};
use x509_parser::prelude::{FromDer, X509Certificate, X509Name};

use super::CaError;

/// What a certificate says of itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateInfo {
    /// `O`: `qd` in every certificate of the CA.
    pub organization: Option<String>,
    /// `OU`: the machine's role.
    pub unit: Option<String>,
    /// `CN`: the machine's name.
    pub common_name: Option<String>,
    /// The serial, lowercase hex, as [`super::IssuedCertificate::serial`].
    pub serial: String,
    pub not_before: DateTime<Utc>,
    pub not_after: DateTime<Utc>,
}

/// The subject's parts, serial and validity of a certificate (PEM). Pure:
/// it checks no signature.
pub fn certificate_info(cert_pem: &str) -> Result<CertificateInfo, CaError> {
    let der = pem_contents(cert_pem, "CERTIFICATE").map_err(CaError::Invalid)?;
    certificate_info_der(&der)
}

/// [`certificate_info`] of a certificate's DER. Nothing may follow it.
pub fn certificate_info_der(der: &[u8]) -> Result<CertificateInfo, CaError> {
    let unparsed = || CaError::Invalid("the certificate cannot be parsed".into());
    let (rest, cert) = X509Certificate::from_der(der).map_err(|_| unparsed())?;
    if !rest.is_empty() {
        return Err(unparsed());
    }
    let time = |t: i64| {
        DateTime::from_timestamp(t, 0)
            .ok_or_else(|| CaError::Invalid("the certificate's dates".into()))
    };
    let subject = cert.subject();
    Ok(CertificateInfo {
        organization: first(subject, &OID_X509_ORGANIZATION_NAME),
        unit: first(subject, &OID_X509_ORGANIZATIONAL_UNIT),
        common_name: first(subject, &OID_X509_COMMON_NAME),
        serial: hex(cert.raw_serial()),
        not_before: time(cert.validity().not_before.timestamp())?,
        not_after: time(cert.validity().not_after.timestamp())?,
    })
}

/// The certificate a machine presented, as Traefik passes it to the API
/// (0066, point 5): its subject, serial and expiry, read with
/// [`certificate_info_der`]. It checks no signature: Traefik verified it.
#[derive(Debug, Clone, Copy, Default)]
pub struct X509CertificateReader;

impl CertificateReader for X509CertificateReader {
    fn read(&self, der: &[u8]) -> Option<MachineCertificate> {
        let info = certificate_info_der(der).ok()?;
        Some(MachineCertificate {
            organization: info.organization,
            unit: info.unit,
            common_name: info.common_name,
            serial: info.serial,
            not_after: SystemTime::from(info.not_after),
        })
    }
}

/// The SHA-256 of a certificate's DER, lowercase hex without separators.
pub fn sha256_fingerprint(cert_pem: &str) -> Result<String, CaError> {
    let der = pem_contents(cert_pem, "CERTIFICATE").map_err(CaError::Invalid)?;
    Ok(hex(&Sha256::digest(&der)))
}

/// The first value of `oid` in a name.
fn first<'a>(name: &X509Name<'a>, oid: &Oid<'a>) -> Option<String> {
    name.iter_by_oid(oid)
        .next()
        .and_then(|v| v.as_str().ok())
        .map(str::to_string)
}

/// The DER inside a PEM block labelled `label`.
pub(super) fn pem_contents(pem: &str, label: &str) -> Result<Vec<u8>, String> {
    let (_, block) = x509_parser::pem::parse_x509_pem(pem.trim().as_bytes())
        .map_err(|_| "not PEM".to_string())?;
    if block.label != label {
        return Err(format!("a PEM {label} was expected"));
    }
    Ok(block.contents)
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
