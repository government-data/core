//! The application VM's machine (0066, points 15 and 16): the installer,
//! on that VM where the CA's key is, makes it `approved` with the role
//! `app` and signs its first certificate on the spot, with no join token
//! and no person, as PostgreSQL needs a certificate before the API starts.
//! From then on its agent renews it as any machine's.
//!
//! An installer may run twice. A second run for the same name makes no
//! second machine: when the certificate already in the agent's directory is
//! the machine's (in use or next), nothing is done; otherwise one is signed
//! again for the same machine and recorded as the one in use.

use auth_gateway::domain::machine::is_label;
use auth_gateway::{Machine, MachineRole, SignedCertificate, SigningRefusal};
use domain::AuditEventType::{CertificateIssued, MachineApproved};

use super::{audit, done, MachinesService, Origin};

/// What the installer gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    /// The certificate held is the machine's already: nothing was done.
    Unchanged(Machine),
    /// A certificate signed now, in use from now on.
    Signed {
        machine: Machine,
        certificate: SignedCertificate,
        /// The CA's certificate, PEM.
        ca: String,
    },
}

/// Why the application VM's machine was not made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    /// The name is not lowercase letters, digits and hyphens, 1 to 63.
    InvalidName,
    /// A machine of another role that is not retired has the name.
    NameTaken,
    /// The signing request is not one the CA signs.
    InvalidRequest,
    /// The installation has no CA.
    Unavailable,
    Backend(String),
}

impl From<domain::RepositoryError> for InstallError {
    fn from(e: domain::RepositoryError) -> Self {
        Self::Backend(e.to_string())
    }
}

impl MachinesService {
    /// The application VM's machine `name`, role `app`, with the signing
    /// request `csr_pem` of the key its agent will keep. `held` is the
    /// serial of the certificate already in the agent's directory, if any.
    /// The machine is made `approved` if no machine has the name; the CA
    /// signs for `O=qd, OU=app, CN=<name>` for the role's term, and the
    /// serial is recorded as the one in use before anything is answered.
    pub async fn install_app(
        &self,
        name: &str,
        csr_pem: &str,
        held: Option<&str>,
        origin: &Origin,
    ) -> Result<Installed, InstallError> {
        let Some(signer) = self.signer.clone() else {
            return Err(InstallError::Unavailable);
        };
        if !is_label(name) {
            return Err(InstallError::InvalidName);
        }
        let role = MachineRole::App;
        let machine = match self.registry.find_live(name).await? {
            Some(m) if m.role != role => return Err(InstallError::NameTaken),
            Some(m) if holds(&m, held) => return Ok(Installed::Unchanged(m)),
            found => found,
        };
        match signer.check(csr_pem, name) {
            Ok(()) => {}
            Err(SigningRefusal::Request(_)) => return Err(InstallError::InvalidRequest),
            Err(SigningRefusal::Failed(e)) => return Err(InstallError::Backend(e)),
        }
        let machine = match machine {
            Some(machine) => machine,
            None => self.create(name, role, origin).await?,
        };
        let signed = signer
            .sign(csr_pem, role, &machine.name, role.certificate_term())
            .map_err(|e| InstallError::Backend(format!("signing: {e:?}")))?;
        let serial = signed.serial.to_ascii_lowercase();
        let change = self
            .registry
            .record_certificate(&machine.id, &serial, signed.not_after)
            .await?;
        let machine = done(change).map_err(|e| InstallError::Backend(format!("{e:?}")))?;
        let event = audit::installed(&machine, CertificateIssued, origin, self.clock.now());
        self.record(event).await?;
        Ok(Installed::Signed {
            machine,
            certificate: SignedCertificate { serial, ..signed },
            ca: signer.ca_pem(),
        })
    }

    /// The machine made `approved`, audited as approved by the installer.
    async fn create(
        &self,
        name: &str,
        role: MachineRole,
        origin: &Origin,
    ) -> Result<Machine, InstallError> {
        let now = self.clock.now();
        let made = self.registry.create_approved(name, role, now).await?;
        let machine = made.ok_or(InstallError::NameTaken)?;
        self.record(audit::installed(&machine, MachineApproved, origin, now))
            .await?;
        Ok(machine)
    }
}

/// Whether `held` is the machine's certificate, in use or next.
fn holds(machine: &Machine, held: Option<&str>) -> bool {
    let Some(held) = held.map(str::to_ascii_lowercase) else {
        return false;
    };
    [&machine.cert_serial, &machine.cert_serial_next]
        .into_iter()
        .any(|s| s.as_deref() == Some(held.as_str()))
}
