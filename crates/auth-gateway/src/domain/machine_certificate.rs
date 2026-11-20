//! The certificate a machine presents (decision 0066, point 5): what the
//! API is told of it, and the port that reads it. Traefik verifies the
//! certificate against the installation's CA; the API only reads the one
//! Traefik verified, and checks its serial against the one the machine uses.

use std::time::SystemTime;

/// The `O` of every certificate the installation's CA signs for a machine.
pub const MACHINE_ORGANIZATION: &str = "qd";

/// What a machine's certificate says of itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineCertificate {
    /// `O`: [`MACHINE_ORGANIZATION`] in every certificate of the CA.
    pub organization: Option<String>,
    /// `OU`: the machine's role.
    pub unit: Option<String>,
    /// `CN`: the machine's name.
    pub common_name: Option<String>,
    /// The serial, lowercase hex.
    pub serial: String,
    pub not_after: SystemTime,
}

/// Port: reads a certificate (DER). Pure: it checks no signature, which is
/// Traefik's to do before the certificate reaches the API.
pub trait CertificateReader: Send + Sync {
    /// The certificate's subject, serial and expiry; `None` if `der` is not
    /// a certificate.
    fn read(&self, der: &[u8]) -> Option<MachineCertificate>;
}
