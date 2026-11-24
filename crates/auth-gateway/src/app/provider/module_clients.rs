//! A module's OIDC clients, as the update agent registers them at `module
//! add` (quality-sdk decision 0009): its people's client `<module>` and its
//! machine client `<module>-machine`. The agent makes each secret and sends
//! it; only its hash is kept, and sending the same one again changes
//! nothing, so the call can run twice. The redirect URIs are made here from
//! the paths and this installation's public origin. A client the request no
//! longer names is disabled; a client registered by hand, or another
//! module's, is never taken over.

use std::time::Duration;

use super::module_client_requests::{machine_client, people_client};
pub use super::module_client_requests::{
    MachineClient, ModuleClientsRequest, PeopleClient, RoleClaimRequest,
};
use super::AuthorizationServer;
use crate::domain::{ClientAuth, ClientId, OidcClient, PreviousSecret};

/// How long a replaced secret is still taken (a rotation).
pub const PREVIOUS_SECRET_TTL: Duration = Duration::from_secs(3600);

/// What a call changed, client by client, for the audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientChange {
    Registered(String),
    Changed(String),
    Disabled(String),
    Deleted(String),
}

/// The answer to a registration: the issuer and the clients' ids, and what
/// changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleClients {
    pub issuer: String,
    pub people_client_id: Option<String>,
    pub machine_client_id: Option<String>,
    pub changes: Vec<ClientChange>,
}

/// Why a registration is refused. Never holds a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleClientsError {
    /// The request is not valid: why.
    Invalid(String),
    /// The id is a client registered by hand or another module's.
    Taken(String),
    Internal(String),
}

impl AuthorizationServer {
    /// Register or update `module`'s clients as `req` says, with redirect
    /// URIs under `origin` (`https://host`), and enable them; a client of
    /// the module it does not name is disabled.
    pub async fn put_module_clients(
        &self,
        module: &str,
        req: &ModuleClientsRequest,
        origin: &str,
    ) -> Result<ModuleClients, ModuleClientsError> {
        let (people_id, machine_id) = client_ids(module)?;
        let mut wanted = Vec::new();
        if let Some(p) = &req.people {
            wanted.push(people_client(module, &people_id, p, origin)?);
        }
        if let Some(m) = &req.machine {
            wanted.push(machine_client(module, &machine_id, m)?);
        }
        let mut changes = Vec::new();
        for (mut client, secret, keep) in wanted {
            let old = self.clients.find(&client.id).await.map_err(internal)?;
            if old.as_ref().is_some_and(|o| !of_module(o, module)) {
                return Err(ModuleClientsError::Taken(client.id.to_string()));
            }
            let hash = self.code_tokens.hash(&secret);
            let previous = self.previous(old.as_ref(), &hash, keep);
            client.auth = ClientAuth::Secret { hash };
            if let Some(m) = client.settings.module.as_mut() {
                m.previous_secret = previous;
            }
            client.settings.check(&client.auth).map_err(invalid)?;
            if let Some(change) = self.save(old.as_ref(), &client).await? {
                changes.push(change);
            }
        }
        let named: Vec<&ClientId> = [&people_id, &machine_id]
            .into_iter()
            .zip([req.people.is_some(), req.machine.is_some()])
            .filter_map(|(id, named)| named.then_some(id))
            .collect();
        changes.extend(self.disable_others(module, &named).await?);
        Ok(ModuleClients {
            issuer: self.issuer.clone(),
            people_client_id: req.people.as_ref().map(|_| people_id.to_string()),
            machine_client_id: req.machine.as_ref().map(|_| machine_id.to_string()),
            changes,
        })
    }

    /// Disable every client of `module`: no code, no token.
    pub async fn disable_module_clients(
        &self,
        module: &str,
    ) -> Result<Vec<ClientChange>, ModuleClientsError> {
        client_ids(module)?;
        self.disable_others(module, &[]).await
    }

    /// Delete every client of `module` (its purge).
    pub async fn delete_module_clients(
        &self,
        module: &str,
    ) -> Result<Vec<ClientChange>, ModuleClientsError> {
        client_ids(module)?;
        let clients = self
            .clients
            .module_clients(module)
            .await
            .map_err(internal)?;
        self.clients
            .delete_module_clients(module)
            .await
            .map_err(internal)?;
        Ok(clients
            .into_iter()
            .map(|c| ClientChange::Deleted(c.id.to_string()))
            .collect())
    }

    /// The previous secret to keep: none unless asked; the one before
    /// `hash` for an hour when it changes; as it was when it does not.
    fn previous(&self, old: Option<&OidcClient>, hash: &str, keep: bool) -> Option<PreviousSecret> {
        let old = old.filter(|_| keep)?;
        let ClientAuth::Secret { hash: old_hash } = &old.auth else {
            return None;
        };
        if old_hash == hash {
            return old.settings.module.as_ref()?.previous_secret.clone();
        }
        Some(PreviousSecret {
            hash: old_hash.clone(),
            until: self.clock.now() + PREVIOUS_SECRET_TTL,
        })
    }

    async fn save(
        &self,
        old: Option<&OidcClient>,
        client: &OidcClient,
    ) -> Result<Option<ClientChange>, ModuleClientsError> {
        if old == Some(client) {
            return Ok(None);
        }
        if !self
            .clients
            .save_module_client(client)
            .await
            .map_err(internal)?
        {
            return Err(ModuleClientsError::Taken(client.id.to_string()));
        }
        let id = client.id.to_string();
        Ok(Some(match old {
            None => ClientChange::Registered(id),
            Some(_) => ClientChange::Changed(id),
        }))
    }

    async fn disable_others(
        &self,
        module: &str,
        keep: &[&ClientId],
    ) -> Result<Vec<ClientChange>, ModuleClientsError> {
        let mut changes = Vec::new();
        for mut client in self
            .clients
            .module_clients(module)
            .await
            .map_err(internal)?
        {
            let Some(m) = client.settings.module.as_mut() else {
                continue;
            };
            if keep.contains(&&client.id) || m.disabled {
                continue;
            }
            m.disabled = true;
            self.clients
                .save_module_client(&client)
                .await
                .map_err(internal)?;
            changes.push(ClientChange::Disabled(client.id.to_string()));
        }
        Ok(changes)
    }
}

fn internal(e: impl std::fmt::Display) -> ModuleClientsError {
    ModuleClientsError::Internal(e.to_string())
}

pub(super) fn invalid(why: &str) -> ModuleClientsError {
    ModuleClientsError::Invalid(why.to_string())
}

fn of_module(client: &OidcClient, module: &str) -> bool {
    client
        .settings
        .module
        .as_ref()
        .is_some_and(|m| m.module == module)
}

/// `<module>` and `<module>-machine`, for a module's name.
fn client_ids(module: &str) -> Result<(ClientId, ClientId), ModuleClientsError> {
    let name_ok = !module.is_empty()
        && module.len() <= 56
        && module
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !name_ok {
        return Err(invalid(
            "a module's name is lower-case letters, digits and -",
        ));
    }
    let parse = |id: String| ClientId::parse(&id).map_err(|_| invalid("module name"));
    Ok((parse(module.into())?, parse(format!("{module}-machine"))?))
}
