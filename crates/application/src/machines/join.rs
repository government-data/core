//! A machine joins (0029, 0066 step 3): it gives its join token and the
//! signing request of the key it made, and gets its first certificate.

use std::time::SystemTime;

use auth_gateway::domain::machine::is_label;
use auth_gateway::{Machine, SigningRefusal};

use super::{JoinError, MachinesService, Origin};

/// What a machine gets when it joins, shown once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Joined {
    /// The machine, `pending` until a person approves it, with its serial.
    pub machine: Machine,
    /// Its certificate, PEM.
    pub certificate: String,
    /// The CA's certificate, PEM.
    pub ca: String,
    pub serial: String,
    pub not_after: SystemTime,
}

impl MachinesService {
    /// A machine named `name` joins with `token` and its signing request
    /// `csr_pem`. The request is checked before the token is touched, so a
    /// bad one is refused and leaves the token unused. Then the token is
    /// used and the machine made `pending` ([`Self::redeem`]); the CA signs
    /// for the token's role, for as long as the role says
    /// (`MachineRole::certificate_term`); and the serial is recorded on the
    /// machine before anything is answered. If signing or recording fails
    /// after the token was used, the answer is an error and no certificate is
    /// given; the token stays used and the machine stays `pending` without a
    /// certificate, for a person to retire before issuing another token.
    pub async fn join(
        &self,
        token: &str,
        name: &str,
        csr_pem: &str,
        origin: &Origin,
    ) -> Result<Joined, JoinError> {
        let Some(signer) = self.signer.clone() else {
            return Err(JoinError::Unavailable);
        };
        if is_label(name) {
            match signer.check(csr_pem, name) {
                Ok(()) => {}
                Err(SigningRefusal::Request(_)) => {
                    let now = self.clock.now();
                    let error = JoinError::InvalidRequest;
                    return Err(self
                        .refuse(name, "invalid-request", error, origin, now)
                        .await);
                }
                Err(SigningRefusal::Failed(e)) => return Err(JoinError::Backend(e)),
            }
        }
        let machine = self.redeem(token, name, origin).await?;
        let term = machine.role.certificate_term();
        let signed = signer
            .sign(csr_pem, machine.role, &machine.name, term)
            .map_err(|e| unfinished(&machine, &format!("signing: {e:?}")))?;
        let machine = self
            .record_certificate(&machine.id, &signed.serial, signed.not_after, origin)
            .await
            .map_err(|e| unfinished(&machine, &format!("recording: {e:?}")))?;
        Ok(Joined {
            machine,
            certificate: signed.pem,
            ca: signer.ca_pem(),
            serial: signed.serial,
            not_after: signed.not_after,
        })
    }
}

/// The token was used but no certificate is given: said in the log, by
/// the machine's name, for the person who will retire it.
fn unfinished(machine: &Machine, why: &str) -> JoinError {
    tracing::error!(
        machine = %machine.name,
        why,
        "a machine joined but got no certificate: it stays pending without one"
    );
    JoinError::Backend(why.to_string())
}
