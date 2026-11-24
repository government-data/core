//! The applications registered with the OIDC provider, in `oidc_clients`
//! ([`PgOidcClients`]): who they are, how they authenticate (no secret, the
//! hash of one, or the role of the machines whose certificate they take,
//! decision 0066) and their settings.

use async_trait::async_trait;
use auth_gateway::{
    ClientAuth, ClientId, ClientSettings, MachineRole, OidcClient, OidcClientRegistry, SigningAlg,
};

use crate::oidc_module_clients::{module_of, MODULE_COLUMNS};
use domain::RepositoryError;
use sqlx::{PgPool, Row};

fn backend(err: sqlx::Error) -> RepositoryError {
    RepositoryError::Backend(err.to_string())
}

/// The registered applications, in `oidc_clients`.
#[derive(Debug, Clone)]
pub struct PgOidcClients {
    pool: PgPool,
}

impl PgOidcClients {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// The `secret_hash` and `machine_role` columns of `auth`.
pub(crate) fn auth_columns(auth: &ClientAuth) -> (Option<&str>, Option<&'static str>) {
    match auth {
        ClientAuth::Public => (None, None),
        ClientAuth::Secret { hash } => (Some(hash.as_str()), None),
        ClientAuth::Certificate { role } => (None, Some(role.as_str())),
    }
}

/// How a stored client authenticates; the table never holds both.
fn auth_of(
    secret_hash: Option<String>,
    machine_role: Option<String>,
) -> Result<ClientAuth, RepositoryError> {
    match (secret_hash, machine_role) {
        (None, None) => Ok(ClientAuth::Public),
        (Some(hash), None) => Ok(ClientAuth::Secret { hash }),
        (None, Some(role)) => MachineRole::parse(&role)
            .map(|role| ClientAuth::Certificate { role })
            .ok_or_else(|| RepositoryError::Backend(format!("stored machine_role {role:?}"))),
        (Some(_), Some(_)) => Err(RepositoryError::Backend(
            "a client with both a secret and a machine role".into(),
        )),
    }
}

#[async_trait]
impl OidcClientRegistry for PgOidcClients {
    async fn find(&self, id: &ClientId) -> Result<Option<OidcClient>, RepositoryError> {
        let row = sqlx::query(&format!(
            "SELECT {CLIENT_COLUMNS}, {MODULE_COLUMNS} FROM oidc_clients WHERE client_id = $1"
        ))
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?;
        row.as_ref().map(client_of).transpose()
    }

    async fn save_module_client(&self, client: &OidcClient) -> Result<bool, RepositoryError> {
        crate::oidc_module_clients::save(&self.pool, client).await
    }

    async fn module_clients(&self, module: &str) -> Result<Vec<OidcClient>, RepositoryError> {
        let rows = sqlx::query(&format!(
            "SELECT {CLIENT_COLUMNS}, {MODULE_COLUMNS} FROM oidc_clients \
             WHERE module = $1 ORDER BY client_id"
        ))
        .bind(module)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        rows.iter().map(client_of).collect()
    }

    async fn delete_module_clients(&self, module: &str) -> Result<u64, RepositoryError> {
        let deleted = sqlx::query("DELETE FROM oidc_clients WHERE module = $1")
            .bind(module)
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(deleted.rows_affected())
    }

    async fn listed(&self) -> Result<Vec<auth_gateway::ListedApp>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT client_id, name, detail, home_url, icon_url FROM oidc_clients \
             WHERE home_url IS NOT NULL ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        rows.iter()
            .map(|r| {
                Ok(auth_gateway::ListedApp {
                    id: r.try_get("client_id").map_err(backend)?,
                    name: r.try_get("name").map_err(backend)?,
                    detail: r.try_get("detail").map_err(backend)?,
                    url: r.try_get("home_url").map_err(backend)?,
                    icon: r.try_get("icon_url").map_err(backend)?,
                })
            })
            .collect()
    }

    async fn insert(&self, client: &OidcClient) -> Result<bool, RepositoryError> {
        let (secret_hash, machine_role) = auth_columns(&client.auth);
        let s = &client.settings;
        let inserted = sqlx::query(
            "INSERT INTO oidc_clients (client_id, name, redirect_uris, secret_hash, token_alg, \
             pkce_required, required_policy, grant_client_credentials, audiences, \
             service_account_id, machine_role) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10::uuid, $11) \
             ON CONFLICT DO NOTHING",
        )
        .bind(client.id.as_str())
        .bind(&client.name)
        .bind(&client.redirect_uris)
        .bind(secret_hash)
        .bind(s.token_alg.as_str())
        .bind(s.pkce_required)
        .bind(&s.required_policy)
        .bind(s.client_credentials)
        .bind(&s.audiences)
        .bind(&s.service_account)
        .bind(machine_role)
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        Ok(inserted.rows_affected() == 1)
    }

    /// Not for a machine role's client, whose settings this does not write:
    /// it is registered with [`Self::insert`].
    async fn upsert(&self, client: &OidcClient) -> Result<(), RepositoryError> {
        let (secret_hash, None) = auth_columns(&client.auth) else {
            return Err(RepositoryError::Backend(
                "a machine role's client is inserted, not upserted".into(),
            ));
        };
        sqlx::query(
            "INSERT INTO oidc_clients (client_id, name, redirect_uris, secret_hash) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (tenant_id, client_id) DO UPDATE SET \
             name = EXCLUDED.name, redirect_uris = EXCLUDED.redirect_uris, \
             secret_hash = EXCLUDED.secret_hash",
        )
        .bind(client.id.as_str())
        .bind(&client.name)
        .bind(&client.redirect_uris)
        .bind(secret_hash)
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        Ok(())
    }
}

/// The columns of a client, before its module's.
const CLIENT_COLUMNS: &str = "client_id, name, redirect_uris, secret_hash, machine_role, \
     token_alg, pkce_required, required_policy, grant_client_credentials, audiences, \
     service_account_id::text AS service_account_id";

/// A client from a row of [`CLIENT_COLUMNS`] and [`MODULE_COLUMNS`].
fn client_of(row: &sqlx::postgres::PgRow) -> Result<OidcClient, RepositoryError> {
    let raw: String = row.try_get("client_id").map_err(backend)?;
    Ok(OidcClient {
        id: ClientId::parse(&raw)
            .map_err(|e| RepositoryError::Backend(format!("stored client {raw:?}: {e}")))?,
        name: row.try_get("name").map_err(backend)?,
        redirect_uris: row.try_get("redirect_uris").map_err(backend)?,
        auth: auth_of(
            row.try_get("secret_hash").map_err(backend)?,
            row.try_get("machine_role").map_err(backend)?,
        )?,
        settings: settings_of(row)?,
    })
}

/// The settings columns of an `oidc_clients` row.
fn settings_of(row: &sqlx::postgres::PgRow) -> Result<ClientSettings, RepositoryError> {
    let alg: String = row.try_get("token_alg").map_err(backend)?;
    Ok(ClientSettings {
        token_alg: SigningAlg::parse(&alg)
            .ok_or_else(|| RepositoryError::Backend(format!("stored token_alg {alg:?}")))?,
        pkce_required: row.try_get("pkce_required").map_err(backend)?,
        required_policy: row.try_get("required_policy").map_err(backend)?,
        client_credentials: row.try_get("grant_client_credentials").map_err(backend)?,
        audiences: row.try_get("audiences").map_err(backend)?,
        service_account: row.try_get("service_account_id").map_err(backend)?,
        module: module_of(row)?,
    })
}
