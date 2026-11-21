//! The installation's CA (decision 0066): it signs the machines'
//! certificates, so that no person issues, copies or renews one.
//!
//! - [`Ca::create`] makes it (EC P-256, `O=qd, CN=<installation> CA`, path
//!   length 0), once, at installation (`server ca-init`, [`write_new`]).
//! - [`Ca::load`] / [`Ca::from_env`] read it back: its certificate from
//!   `CA_CERT_FILE` and its key, a file secret, from `CA_KEY_FILE`.
//! - [`Ca::sign`] signs a machine's request: it keeps only the request's
//!   key, and writes the subject, names and uses itself ([`sign`]).
//! - [`certificate_info`] reads a certificate's subject, serial and expiry.
//! - The CA is the join's `auth_gateway::CertificateSigner` (`signer.rs`).
//!
//! Built with `rcgen` over `ring`; no OpenSSL. The key is never logged: no
//! `Debug` here prints it.

mod files;
mod inspect;
mod sign;
mod signer;
#[cfg(test)]
mod tests;
pub mod updater_files;

use std::time::Duration;

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::oidc_es256::key_from_env;

pub use files::write_new;
pub use inspect::{
    certificate_info, certificate_info_der, sha256_fingerprint, CertificateInfo,
    X509CertificateReader,
};
pub use sign::{IssuedCertificate, Lifetime, MachineIdentity};
pub use updater_files::UpdaterFiles;

/// The variable of the CA's certificate file (PEM).
pub const CA_CERT_VAR: &str = "CA_CERT";
/// The CA's key (PKCS#8 PEM), a secret read only from `CA_KEY_FILE`.
pub const CA_KEY_VAR: &str = "CA_KEY";
/// The CA's validity unless the caller says otherwise: 10 years.
pub const DEFAULT_CA_VALIDITY: Duration = Duration::from_secs(10 * 365 * 24 * 3600);
/// How far back a certificate starts, for clocks a little behind.
const CLOCK_SKEW: time::Duration = time::Duration::minutes(5);
/// The organization of every subject the CA writes.
const ORGANIZATION: &str = "qd";

/// Why the CA could not do what it was asked.
#[derive(Debug)]
pub enum CaError {
    /// What the caller gave is not valid: a name, a role, a host name, a
    /// lifetime.
    Invalid(String),
    /// The machine's signing request is refused.
    Request(String),
    /// The CA's certificate or key cannot be read, or they do not match.
    Load(String),
    /// A file of the CA exists already, or cannot be written.
    File(String),
    /// Building or signing a certificate failed.
    Build(rcgen::Error),
}

impl std::fmt::Display for CaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(m) => write!(f, "invalid: {m}"),
            Self::Request(m) => write!(f, "signing request refused: {m}"),
            Self::Load(m) => write!(f, "CA: {m}"),
            Self::File(m) => write!(f, "CA file: {m}"),
            Self::Build(e) => write!(f, "certificate: {e}"),
        }
    }
}

impl std::error::Error for CaError {}

impl From<rcgen::Error> for CaError {
    fn from(e: rcgen::Error) -> Self {
        Self::Build(e)
    }
}

/// A newly made CA: its certificate and its key, both PEM. The key is
/// PKCS#8 and is a secret; `Debug` does not print it.
pub struct NewCa {
    pub cert_pem: String,
    pub key_pem: String,
}

impl std::fmt::Debug for NewCa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewCa").finish_non_exhaustive()
    }
}

/// The installation's CA, ready to sign.
pub struct Ca {
    issuer: Issuer<'static, KeyPair>,
    cert_pem: String,
}

impl std::fmt::Debug for Ca {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ca").finish_non_exhaustive()
    }
}

impl Ca {
    /// A new CA for `installation` (subject `O=qd, CN=<installation> CA`),
    /// valid for `validity` from now: EC P-256, `CA:TRUE` with path length
    /// 0, used only to sign certificates and revocation lists.
    pub fn create(installation: &str, validity: Duration) -> Result<NewCa, CaError> {
        let name = installation.trim();
        if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
            return Err(CaError::Invalid(
                "the installation's name: 1 to 60 characters, none of control".into(),
            ));
        }
        let now = time::OffsetDateTime::now_utc();
        let not_after = time::Duration::try_from(validity)
            .ok()
            .filter(|v| v.is_positive())
            .and_then(|v| now.checked_add(v))
            .ok_or_else(|| CaError::Invalid("the CA's validity".into()))?;
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::OrganizationName, ORGANIZATION);
        params
            .distinguished_name
            .push(DnType::CommonName, format!("{name} CA"));
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params.serial_number = Some(sign::random_serial()?);
        params.not_before = now - CLOCK_SKEW;
        params.not_after = not_after;
        let cert = params.self_signed(&key)?;
        Ok(NewCa {
            cert_pem: cert.pem(),
            key_pem: key.serialize_pem(),
        })
    }

    /// The CA from its certificate and its key (PKCS#8), both PEM. The key
    /// must be P-256 and the certificate's, and the certificate a CA's.
    pub fn load(cert_pem: &str, key_pem: &str) -> Result<Self, CaError> {
        let key = KeyPair::from_pem(key_pem)
            .map_err(|_| CaError::Load("the key is not a PKCS#8 PEM key".into()))?;
        if key.algorithm() != &PKCS_ECDSA_P256_SHA256 {
            return Err(CaError::Load("the key is not EC P-256".into()));
        }
        let der = inspect::pem_contents(cert_pem, "CERTIFICATE")
            .map_err(|e| CaError::Load(format!("the certificate: {e}")))?;
        let (_, cert) = X509Certificate::from_der(&der)
            .map_err(|_| CaError::Load("the certificate cannot be parsed".into()))?;
        if !cert.is_ca() {
            return Err(CaError::Load("the certificate is not a CA's".into()));
        }
        if cert.public_key().subject_public_key.data.as_ref() != key.public_key_raw() {
            return Err(CaError::Load("the key is not the certificate's".into()));
        }
        let issuer = Issuer::from_ca_cert_der(&der.into(), key)
            .map_err(|_| CaError::Load("the certificate cannot be read as an issuer".into()))?;
        Ok(Self {
            issuer,
            cert_pem: cert_pem.to_string(),
        })
    }

    /// The CA of `CA_CERT_FILE` (or `CA_CERT` itself) and `CA_KEY_FILE`: the
    /// key only from its file, never a variable (0066, point 3). `None` if
    /// neither is set, an error if only one is.
    pub fn from_env() -> Result<Option<Self>, CaError> {
        if std::env::var_os(CA_KEY_VAR).is_some() {
            return Err(CaError::Load(format!(
                "{CA_KEY_VAR} is read only from a file: {CA_KEY_VAR}_FILE"
            )));
        }
        let cert = key_from_env(CA_CERT_VAR).map_err(CaError::Load)?;
        let key = key_from_env(CA_KEY_VAR).map_err(CaError::Load)?;
        match (cert, key) {
            (None, None) => Ok(None),
            (Some(cert), Some(key)) => Self::load(&cert, &key).map(Some),
            _ => Err(CaError::Load(format!(
                "{CA_CERT_VAR}_FILE and {CA_KEY_VAR}_FILE go together"
            ))),
        }
    }

    /// The CA's certificate, PEM: what machines and services trust.
    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }
}
