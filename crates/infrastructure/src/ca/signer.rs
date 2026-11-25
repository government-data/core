//! The CA as the join's [`CertificateSigner`] (0066, step 3): a machine's
//! certificate is for `O=qd, OU=<role>, CN=<name>` with `name` as its
//! alternative name, for 24 hours or the one-year ceiling as the role says.
//! The application machine's also carries [`DOLT_NAME`]: Dolt serves it, and
//! in the core's package the API reaches Dolt by that name and verifies it
//! (decision 0080's amendment, point 9), the machine's own name being
//! PostgreSQL's there.

use std::time::SystemTime;

use auth_gateway::{
    CertificateSigner, CertificateTerm, MachineRole, SignedCertificate, SigningRefusal,
};

use super::sign::request_key;
use super::{Ca, CaError, Lifetime, MachineIdentity};

/// The application machine's second name: Dolt's in the core's package.
pub(crate) const DOLT_NAME: &str = "dolt";

fn identity(role: MachineRole, name: &str) -> Result<MachineIdentity, SigningRefusal> {
    let names: &[&str] = match role {
        MachineRole::App => &[name, DOLT_NAME],
        _ => &[name],
    };
    MachineIdentity::new(role.as_str(), name, names).map_err(refusal)
}

/// What the caller gave is the request's fault; anything else, the CA's.
fn refusal(e: CaError) -> SigningRefusal {
    match e {
        CaError::Invalid(m) | CaError::Request(m) => SigningRefusal::Request(m),
        other => SigningRefusal::Failed(other.to_string()),
    }
}

fn lifetime(term: CertificateTerm) -> Lifetime {
    match term {
        CertificateTerm::Day => Lifetime::MACHINE,
        CertificateTerm::UntilReplaced => Lifetime::CEILING,
    }
}

impl CertificateSigner for Ca {
    fn check(&self, csr_pem: &str, name: &str) -> Result<(), SigningRefusal> {
        // Every role is a label: the name decides whether it can be signed.
        identity(MachineRole::Worker, name)?;
        request_key(csr_pem).map(|_| ()).map_err(refusal)
    }

    fn sign(
        &self,
        csr_pem: &str,
        role: MachineRole,
        name: &str,
        term: CertificateTerm,
    ) -> Result<SignedCertificate, SigningRefusal> {
        let issued =
            Ca::sign(self, csr_pem, &identity(role, name)?, lifetime(term)).map_err(refusal)?;
        Ok(SignedCertificate {
            pem: issued.pem,
            serial: issued.serial,
            not_after: SystemTime::from(issued.not_after),
        })
    }

    fn ca_pem(&self) -> String {
        self.cert_pem().to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};

    use super::*;
    use crate::ca::{certificate_info, DEFAULT_CA_VALIDITY};

    fn ca() -> Ca {
        let made = Ca::create("test", DEFAULT_CA_VALIDITY).unwrap();
        Ca::load(&made.cert_pem, &made.key_pem).unwrap()
    }

    fn request() -> String {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let params = CertificateParams::new(vec!["other.example".to_string()]).unwrap();
        params.serialize_request(&key).unwrap().pem().unwrap()
    }

    #[test]
    fn ca_signer_checks_before_and_signs_by_term() {
        let ca = ca();
        let csr = request();
        assert_eq!(ca.check(&csr, "worker-1"), Ok(()));
        assert!(matches!(
            ca.check("not a request", "worker-1"),
            Err(SigningRefusal::Request(_))
        ));
        // A label, but not a host name: it cannot be the alternative name.
        assert!(matches!(
            ca.check(&csr, "-worker"),
            Err(SigningRefusal::Request(_))
        ));
        let now = SystemTime::now();
        for (term, days) in [
            (CertificateTerm::Day, 1),
            (CertificateTerm::UntilReplaced, 365),
        ] {
            // `Ca::sign` is the CA's own; this is the port's.
            let role = MachineRole::TrinoCoordinator;
            let signed = CertificateSigner::sign(&ca, &csr, role, "trino-1", term).unwrap();
            let info = certificate_info(&signed.pem).unwrap();
            assert_eq!(info.unit.as_deref(), Some("trino-coordinator"));
            assert_eq!(info.common_name.as_deref(), Some("trino-1"));
            assert_eq!(info.serial, signed.serial);
            let ends = now + Duration::from_secs(days * 24 * 3600);
            let off = signed
                .not_after
                .duration_since(ends)
                .unwrap_or_else(|e| e.duration());
            assert!(off < Duration::from_secs(60), "{term:?}");
        }
        assert_eq!(ca.ca_pem(), ca.cert_pem());
    }

    /// The DNS names a certificate serves under.
    fn dns_names(pem: &str) -> Vec<String> {
        use x509_parser::extensions::GeneralName;
        use x509_parser::prelude::{FromDer, X509Certificate};
        let der = crate::ca::inspect::pem_contents(pem, "CERTIFICATE").unwrap();
        let (_, cert) = X509Certificate::from_der(&der).unwrap();
        let names = cert.subject_alternative_name().unwrap().unwrap();
        names
            .value
            .general_names
            .iter()
            .filter_map(|n| match n {
                GeneralName::DNSName(d) => Some((*d).to_string()),
                _ => None,
            })
            .collect()
    }

    fn signed_names(role: MachineRole) -> Vec<String> {
        let term = role.certificate_term();
        let signed = CertificateSigner::sign(&ca(), &request(), role, "qd-app", term).unwrap();
        dns_names(&signed.pem)
    }

    #[test]
    fn the_application_machine_is_also_dolt() {
        assert_eq!(signed_names(MachineRole::App), ["qd-app", DOLT_NAME]);
    }

    #[test]
    fn no_other_role_is_dolt() {
        for role in MachineRole::ALL
            .into_iter()
            .filter(|r| *r != MachineRole::App)
        {
            assert_eq!(signed_names(role), ["qd-app"], "{role:?}");
        }
    }
}
