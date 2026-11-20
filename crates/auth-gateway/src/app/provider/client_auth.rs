//! Who is asking at `/token`: a public client (no credential), a client with
//! a secret, or a machine role's client, authenticated by the certificate of
//! one of its machines (decision 0066, point 6; RFC 8705 `tls_client_auth`).
//! Every refusal is `invalid_client`, with no detail of which check failed.

use super::{AuthorizationServer, TokenError, TokenRequest};
use crate::app::machines::accept_certificate;
use crate::domain::{
    ClientAuth, ClientId, Machine, MachineCertificate, MachineRole, OidcClient,
    MACHINE_ORGANIZATION,
};

impl AuthorizationServer {
    /// The registered client a `/token` request names.
    pub(super) async fn requested_client(
        &self,
        req: &TokenRequest,
    ) -> Result<OidcClient, TokenError> {
        let id = req
            .client_id
            .as_deref()
            .and_then(|c| ClientId::parse(c).ok())
            .ok_or(TokenError::InvalidClient)?;
        self.clients
            .find(&id)
            .await
            .map_err(|e| TokenError::Internal(e.to_string()))?
            .ok_or(TokenError::InvalidClient)
    }

    /// The client of a `/token` request, if it is public and sent no secret
    /// or sent its own. A machine role's client is never authenticated here:
    /// only [`Self::certified_machine`] does, for `client_credentials`.
    pub(super) async fn authenticate_client(
        &self,
        req: &TokenRequest,
    ) -> Result<OidcClient, TokenError> {
        let client = self.requested_client(req).await?;
        self.check_secret(&client, req)?;
        Ok(client)
    }

    /// Whether `req` carries what `client`, public or with a secret, needs.
    pub(super) fn check_secret(
        &self,
        client: &OidcClient,
        req: &TokenRequest,
    ) -> Result<(), TokenError> {
        match (&client.auth, req.client_secret.as_deref()) {
            (ClientAuth::Public, None) => Ok(()),
            (ClientAuth::Secret { hash }, Some(secret))
                if &self.code_tokens.hash(secret) == hash =>
            {
                Ok(())
            }
            _ => Err(TokenError::InvalidClient),
        }
    }

    /// The machine whose certificate came with `req`, if it authenticates a
    /// client of `role`: no secret sent; the certificate's `O=qd` and its
    /// `OU` the role; the registry accepts its name and serial (the one in
    /// use or the next one, which then becomes the one in use); and the
    /// machine, of that role, is approved or draining.
    pub(super) async fn certified_machine(
        &self,
        role: MachineRole,
        req: &TokenRequest,
    ) -> Result<Machine, TokenError> {
        let (Some(machines), Some(cert), None) =
            (&self.machines, &req.client_certificate, &req.client_secret)
        else {
            return Err(TokenError::InvalidClient);
        };
        if !of_role(cert, role) {
            return Err(TokenError::InvalidClient);
        }
        let name = cert.common_name.as_deref().unwrap_or_default();
        let machine = accept_certificate(machines.as_ref(), name, &cert.serial)
            .await
            .map_err(|e| TokenError::Internal(e.to_string()))?
            .ok_or(TokenError::InvalidClient)?;
        if machine.role != role || !machine.state.gets_tokens() {
            return Err(TokenError::InvalidClient);
        }
        Ok(machine)
    }
}

/// Whether the certificate says it is the installation's, for `role`.
fn of_role(cert: &MachineCertificate, role: MachineRole) -> bool {
    cert.organization.as_deref() == Some(MACHINE_ORGANIZATION)
        && cert.unit.as_deref() == Some(role.as_str())
}
