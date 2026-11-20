//! The CA as the join's [`CertificateSigner`] (0066, step 3): a machine's
//! certificate is for `O=qd, OU=<role>, CN=<name>` with `name` as its only
//! alternative name, for 24 hours or the one-year ceiling as the role says.

use std::time::SystemTime;

use auth_gateway::{
    CertificateSigner, CertificateTerm, MachineRole, SignedCertificate, SigningRefusal,
};

use super::sign::request_key;
use super::{Ca, CaError, Lifetime, MachineIdentity};

fn identity(role: MachineRole, name: &str) -> Result<MachineIdentity, SigningRefusal> {
    MachineIdentity::new(role.as_str(), name, [name]).map_err(refusal)
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
}
