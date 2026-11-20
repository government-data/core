//! Signing a machine's request (0066, point 2): of the request only its key
//! is kept, after checking that the request is signed by it and that it is
//! EC P-256. Subject, names, uses and dates are the CA's.

use std::net::IpAddr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rcgen::string::Ia5String;
use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyUsagePurpose,
    PublicKeyData, SanType, SerialNumber, SignatureAlgorithm, PKCS_ECDSA_P256_SHA256,
};
use ring::rand::{SecureRandom, SystemRandom};
use x509_parser::certification_request::X509CertificationRequest;
use x509_parser::oid_registry::{OID_EC_P256, OID_KEY_TYPE_EC_PUBLIC_KEY};
use x509_parser::prelude::FromDer;

use super::inspect::{hex, pem_contents};
use super::{Ca, CaError, CLOCK_SKEW, ORGANIZATION};

/// How long a certificate lasts: at most one year, which is the ceiling of
/// a certificate taken only at a restart (Trino, Dolt: 0066, point 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lifetime(Duration);

impl Lifetime {
    /// A machine's certificate: 24 hours, renewed every 8.
    pub const MACHINE: Self = Self(Duration::from_secs(24 * 3600));
    /// The ceiling: one year.
    pub const CEILING: Self = Self(Duration::from_secs(365 * 24 * 3600));

    /// A lifetime of `duration`: more than nothing, at most [`Self::CEILING`].
    pub fn new(duration: Duration) -> Result<Self, CaError> {
        if duration.is_zero() || duration > Self::CEILING.0 {
            return Err(CaError::Invalid(
                "a certificate's lifetime: more than zero, at most one year".into(),
            ));
        }
        Ok(Self(duration))
    }

    pub fn duration(self) -> Duration {
        self.0
    }
}

/// Who a certificate is for: `O=qd, OU=<role>, CN=<name>`, and the names
/// (DNS or IP) it serves under.
#[derive(Debug, Clone)]
pub struct MachineIdentity {
    role: String,
    name: String,
    alt_names: Vec<SanType>,
}

impl MachineIdentity {
    /// Role and name: lowercase letters, digits and hyphens, 1 to 63. Each
    /// alternative name a host name or an IP address; there may be none.
    pub fn new<I>(role: &str, name: &str, alt_names: I) -> Result<Self, CaError>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        for (what, value) in [("role", role), ("machine name", name)] {
            if !is_label(value) {
                return Err(CaError::Invalid(format!(
                    "the {what} {value:?}: lowercase letters, digits and hyphens, 1 to 63"
                )));
            }
        }
        let alt_names = alt_names
            .into_iter()
            .map(|n| alt_name(n.as_ref()))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            role: role.to_string(),
            name: name.to_string(),
            alt_names,
        })
    }
}

/// A signed certificate.
#[derive(Debug, Clone)]
pub struct IssuedCertificate {
    /// The certificate, PEM.
    pub pem: String,
    /// Its serial, lowercase hex (what `machines` keeps).
    pub serial: String,
    pub not_after: DateTime<Utc>,
}

impl Ca {
    /// Sign `csr_pem` (PKCS#10) for `machine`, valid from five minutes ago
    /// (clocks behind) for `lifetime`: client and server use, `CA:FALSE`, a
    /// random serial. The request's subject and extensions are ignored.
    pub fn sign(
        &self,
        csr_pem: &str,
        machine: &MachineIdentity,
        lifetime: Lifetime,
    ) -> Result<IssuedCertificate, CaError> {
        let key = request_key(csr_pem)?;
        let now = time::OffsetDateTime::now_utc();
        let not_after = time::Duration::try_from(lifetime.0)
            .ok()
            .and_then(|l| now.checked_add(l))
            .ok_or_else(|| CaError::Invalid("a certificate's lifetime".into()))?;
        let serial = random_serial()?;
        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::OrganizationName, ORGANIZATION);
        params
            .distinguished_name
            .push(DnType::OrganizationalUnitName, machine.role.as_str());
        params
            .distinguished_name
            .push(DnType::CommonName, machine.name.as_str());
        params.subject_alt_names = machine.alt_names.clone();
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ClientAuth,
            ExtendedKeyUsagePurpose::ServerAuth,
        ];
        params.use_authority_key_identifier_extension = true;
        params.serial_number = Some(serial.clone());
        params.not_before = now - CLOCK_SKEW;
        params.not_after = not_after;
        let cert = params.signed_by(&key, &self.issuer)?;
        Ok(IssuedCertificate {
            pem: cert.pem(),
            serial: hex(&serial.to_bytes()),
            // The certificate holds whole seconds.
            not_after: DateTime::from_timestamp(not_after.unix_timestamp(), 0)
                .ok_or_else(|| CaError::Invalid("a certificate's lifetime".into()))?,
        })
    }
}

/// The public key of a request, once it is known to be P-256 and to have
/// signed the request.
pub(super) struct RequestKey(Vec<u8>);

impl PublicKeyData for RequestKey {
    fn der_bytes(&self) -> &[u8] {
        &self.0
    }

    fn algorithm(&self) -> &'static SignatureAlgorithm {
        &PKCS_ECDSA_P256_SHA256
    }
}

pub(super) fn request_key(csr_pem: &str) -> Result<RequestKey, CaError> {
    let der = pem_contents(csr_pem, "CERTIFICATE REQUEST").map_err(CaError::Request)?;
    let (rest, csr) = X509CertificationRequest::from_der(&der)
        .map_err(|_| CaError::Request("it cannot be parsed".into()))?;
    if !rest.is_empty() {
        return Err(CaError::Request("bytes after the request".into()));
    }
    let spki = &csr.certification_request_info.subject_pki;
    let curve = spki
        .algorithm
        .parameters
        .as_ref()
        .and_then(|p| p.as_oid().ok());
    let point = spki.subject_public_key.data.as_ref();
    // An uncompressed point: 0x04 || x || y, 32 bytes each.
    if spki.algorithm.algorithm != OID_KEY_TYPE_EC_PUBLIC_KEY
        || curve != Some(OID_EC_P256)
        || point.len() != 65
        || point[0] != 4
    {
        return Err(CaError::Request("its key is not EC P-256".into()));
    }
    csr.verify_signature()
        .map_err(|_| CaError::Request("its signature does not verify".into()))?;
    Ok(RequestKey(point.to_vec()))
}

/// 128 bits, positive and of fixed length: the first bit 0, the second 1,
/// the other 126 random.
pub(super) fn random_serial() -> Result<SerialNumber, CaError> {
    let mut bytes = [0u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| CaError::Build(rcgen::Error::RingUnspecified))?;
    bytes[0] = (bytes[0] & 0x7f) | 0x40;
    Ok(SerialNumber::from_slice(&bytes))
}

/// Lowercase letters, digits and hyphens, 1 to 63.
fn is_label(value: &str) -> bool {
    (1..=63).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// An IP address, or a host name: labels of letters, digits and hyphens,
/// none starting or ending with a hyphen, 253 characters at most. No
/// wildcards.
fn alt_name(value: &str) -> Result<SanType, CaError> {
    if let Ok(ip) = value.parse::<IpAddr>() {
        return Ok(SanType::IpAddress(ip));
    }
    let host = value.to_ascii_lowercase();
    let valid = host.len() <= 253
        && host
            .split('.')
            .all(|label| is_label(label) && !label.starts_with('-') && !label.ends_with('-'));
    if !valid {
        return Err(CaError::Invalid(format!(
            "{value:?} is neither a host name nor an IP address"
        )));
    }
    Ia5String::try_from(host)
        .map(SanType::DnsName)
        .map_err(|_| CaError::Invalid(format!("{value:?} is not ASCII")))
}
