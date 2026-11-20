//! The steps that touch the API, the files and the commands.

use crate::api::{CallError, Issued, MachineApi};
use crate::cert::{self, Credentials, Request, Validity};
use crate::config::JoinToken;
use crate::plan::Mode;
use crate::ports::{Clock, Commands, Renewal};

use super::{human, Agent, Fatal};

/// The name a renewal's request carries when none is set: the CA keeps the
/// machine's own anyway.
const ANY_NAME: &str = "machine";

impl<A: MachineApi, C: Clock, R: Commands> Agent<A, C, R> {
    /// Exchanges the join token for the first certificate.
    pub(super) async fn join(&mut self) -> Result<(), Fatal> {
        let s = &self.settings;
        let missing = |name: &str| Fatal::Settings(format!("{name} is needed to join"));
        let name = s.name.clone().ok_or_else(|| missing("QD_MACHINE_NAME"))?;
        let expected = s.ca_sha256.clone().ok_or_else(|| missing("QD_CA_SHA256"))?;
        s.public_url
            .as_ref()
            .ok_or_else(|| missing("QD_PUBLIC_URL"))?;
        let token = s
            .token
            .clone()
            .ok_or_else(|| missing("QD_JOIN_TOKEN_FILE"))?;
        let request = cert::new_request(&name).map_err(Fatal::Answer)?;
        let secret = token.read().map_err(Fatal::Settings)?;
        let issued = match self.api.join(&secret, &name, &request.csr_pem).await {
            Ok(issued) => issued,
            Err(CallError::Refused) => return Err(Fatal::JoinRefused),
            Err(CallError::NameTaken) => return Err(Fatal::NameTaken),
            Err(e) if e.retryable() => {
                self.failed("joining", &e, None);
                return Ok(());
            }
            Err(e) => return Err(Fatal::Rejected(e)),
        };
        if !cert::is_ca(&issued.ca, &expected) {
            return Err(Fatal::CaMismatch);
        }
        let (credentials, _) = checked(&request, &issued)?;
        self.store.write_ca(&issued.ca)?;
        self.store.store_next(&credentials)?;
        tracing::info!(machine = %name, serial = %issued.serial, state = %issued.state, "joined");
        if let JoinToken::File(path) = &token {
            if let Err(e) = std::fs::remove_file(path) {
                tracing::warn!(file = %path.display(), "the used join token's file stays: {e}");
            }
        }
        self.succeeded();
        self.promote().await
    }

    /// A new certificate, kept as the next one.
    pub(super) async fn renew(&mut self, current: Option<Validity>) -> Result<(), Fatal> {
        let (Some(identity), Some(ca)) = (self.store.current()?, self.store.ca()?) else {
            return Err(Fatal::Settings(
                "renewing needs machine.pem and ca.crt".into(),
            ));
        };
        let name = self.settings.name.as_deref().unwrap_or(ANY_NAME);
        let request = cert::new_request(name).map_err(Fatal::Answer)?;
        let issued = match self.api.renew(&identity, &ca, &request.csr_pem).await {
            Ok(issued) => issued,
            Err(CallError::Refused) => {
                self.renewals.renewed(Renewal::Refused);
                self.store.mark_refused()?;
                return Err(Fatal::Refused);
            }
            Err(e) if e.retryable() => {
                // Lost or not, the certificate in use is still accepted
                // (0066, point 14): it is kept, and the renewal asked again.
                self.renewals.renewed(Renewal::Failed);
                self.failed("renewing", &e, current);
                return Ok(());
            }
            Err(e) => {
                self.renewals.renewed(Renewal::Failed);
                return Err(Fatal::Rejected(e));
            }
        };
        let kept = self.keep(&request, &issued, &ca);
        let how = if kept.is_ok() {
            Renewal::Ok
        } else {
            Renewal::Failed
        };
        self.renewals.renewed(how);
        let lasts = human(kept?.lifetime());
        tracing::info!(serial = %issued.serial, state = %issued.state, lasts = %lasts, "renewed");
        self.succeeded();
        Ok(())
    }

    /// The renewed certificate kept as the next one, once it is of the CA
    /// and of the key sent.
    fn keep(&self, request: &Request, issued: &Issued, ca: &str) -> Result<Validity, Fatal> {
        if cert::fingerprint(&issued.ca).ok() != cert::fingerprint(ca).ok() {
            return Err(Fatal::CaChanged);
        }
        let (next, validity) = checked(request, issued)?;
        self.store.store_next(&next)?;
        Ok(validity)
    }

    /// Makes the next certificate the one in use, then runs the hook.
    pub(super) async fn promote(&mut self) -> Result<(), Fatal> {
        if self.store.promote()? {
            self.ask_at = self.clock.now();
            tracing::info!("the new certificate is in place");
            self.after_renew().await;
        }
        Ok(())
    }

    /// In held mode: may the next certificate be taken now?
    pub(super) async fn ask_ready(&mut self) -> Result<(), Fatal> {
        let Mode::Held { ask_every, .. } = self.settings.mode else {
            return Ok(());
        };
        let Some(ready) = self.settings.held_ready.clone() else {
            return Err(Fatal::Settings("QD_HELD_READY is needed".into()));
        };
        match self.commands.run(&ready).await {
            Ok(true) => {
                tracing::info!("QD_HELD_READY: ready, taking the next certificate");
                return self.promote().await;
            }
            Ok(false) => tracing::info!(again_in = %human(ask_every), "QD_HELD_READY: not yet"),
            Err(e) => tracing::warn!(again_in = %human(ask_every), "QD_HELD_READY: {e}"),
        }
        self.ask_at = self.clock.now() + ask_every;
        Ok(())
    }
}

/// The key with the certificate answered, once it is of that key.
fn checked(request: &Request, issued: &Issued) -> Result<(Credentials, Validity), Fatal> {
    let credentials = request.credentials(&issued.certificate);
    let validity = credentials.validity().map_err(Fatal::Answer)?;
    Ok((credentials, validity))
}
