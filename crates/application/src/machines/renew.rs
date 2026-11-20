//! A machine renews its certificate with the one it has (0066, steps 3 and
//! 4): it gives the signing request of a new key and gets a certificate for
//! the same name and role, for as long as its role says.
//!
//! The renewed certificate is the machine's next one (point 14): the one it
//! presented stays accepted until the next one is first presented, so a
//! machine that renews and loses the answer still works with the one it has,
//! and renews again, which replaces the next one.

use std::time::SystemTime;

use auth_gateway::{Machine, MachineCertificate, SigningRefusal, MACHINE_ORGANIZATION};

use super::{audit, done, Acceptance, MachinesError, MachinesService, Origin};

/// What a machine gets when it renews, shown once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renewed {
    /// The machine, with the new certificate as its next one.
    pub machine: Machine,
    /// Its new certificate, PEM.
    pub certificate: String,
    /// The CA's certificate, PEM.
    pub ca: String,
    pub serial: String,
    pub not_after: SystemTime,
}

/// Why a machine could not renew.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenewError {
    /// The certificate is unknown, replaced, of a retired machine or of
    /// another role: one answer for all of them.
    Refused,
    /// The signing request is not one the CA signs.
    InvalidRequest,
    /// The installation has no CA.
    Unavailable,
    Backend(String),
}

impl From<MachinesError> for RenewError {
    fn from(e: MachinesError) -> Self {
        match e {
            MachinesError::Backend(e) => Self::Backend(e),
            MachinesError::Unavailable => Self::Unavailable,
            _ => Self::Refused,
        }
    }
}

impl MachinesService {
    /// The machine of `presented`, the certificate Traefik verified, renews
    /// with the signing request `csr_pem`. It must be [`Self::accepts`]'s
    /// (not retired, the serial in use or the next one) and of the role the
    /// machine has. The CA signs for the same name and role, for the role's
    /// term, and the new serial is recorded as the next one before anything
    /// is answered. Every refusal is audited; no certificate is.
    pub async fn renew(
        &self,
        presented: &MachineCertificate,
        csr_pem: &str,
        origin: &Origin,
    ) -> Result<Renewed, RenewError> {
        let Some(signer) = self.signer.clone() else {
            return Err(RenewError::Unavailable);
        };
        let name = presented.common_name.as_deref().unwrap_or_default();
        let refuse = |reason, error| self.renew_refused(Some(name), reason, error, origin);
        let machine = match self.accepts(name, &presented.serial).await? {
            Acceptance::Accepted(machine) => *machine,
            Acceptance::Refused => return Err(refuse("certificate", RenewError::Refused).await),
        };
        let organization = presented.organization.as_deref();
        if organization != Some(MACHINE_ORGANIZATION)
            || presented.unit.as_deref() != Some(machine.role.as_str())
        {
            return Err(refuse("role", RenewError::Refused).await);
        }
        match signer.check(csr_pem, &machine.name) {
            Ok(()) => {}
            Err(SigningRefusal::Request(_)) => {
                return Err(refuse("invalid-request", RenewError::InvalidRequest).await)
            }
            Err(SigningRefusal::Failed(e)) => return Err(RenewError::Backend(e)),
        }
        let term = machine.role.certificate_term();
        let signed = signer
            .sign(csr_pem, machine.role, &machine.name, term)
            .map_err(|e| RenewError::Backend(format!("signing: {e:?}")))?;
        let serial = signed.serial.to_ascii_lowercase();
        let change = self
            .registry
            .record_next_certificate(&machine.id, &serial, signed.not_after)
            .await
            .map_err(MachinesError::from)?;
        // Retired between the check and now: no certificate is given.
        let machine = done(change)?;
        let now = self.clock.now();
        self.record(audit::certificate_renewed(&machine, origin, now))
            .await
            .map_err(MachinesError::from)?;
        Ok(Renewed {
            machine,
            certificate: signed.pem,
            ca: signer.ca_pem(),
            serial,
            not_after: signed.not_after,
        })
    }

    /// A renewal refused before any certificate was read (none was
    /// presented, or it could not be read): audited, and refused.
    pub async fn renew_without_certificate(&self, origin: &Origin) {
        let refused = RenewError::Refused;
        self.renew_refused(None, "no-certificate", refused, origin)
            .await;
    }

    /// A refusal is audited, and answered whether or not it could be.
    async fn renew_refused(
        &self,
        name: Option<&str>,
        reason: &str,
        error: RenewError,
        origin: &Origin,
    ) -> RenewError {
        let event = audit::renew_refused(name, reason, origin, self.clock.now());
        if let Err(e) = self.record(event).await {
            tracing::error!(error = %e, "a refused renewal could not be audited");
        }
        error
    }
}
