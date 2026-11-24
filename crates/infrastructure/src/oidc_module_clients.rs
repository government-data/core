//! The module side of the clients in `oidc_clients` (quality-sdk decision
//! 0009, migration 0030): read with every client, written whole when a
//! module registers or changes its clients. A row with `module` null was
//! registered by hand.

use std::time::SystemTime;

use auth_gateway::{ModuleClient, OidcClient, PreviousSecret, RoleClaim, RoleValue};
use chrono::{DateTime, Utc};
use domain::RepositoryError;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};

use crate::oidc_clients::auth_columns;

/// The module columns, as [`module_of`] reads them.
pub(crate) const MODULE_COLUMNS: &str = "module, disabled_at, previous_secret_hash, \
     previous_secret_until, role_claim, consumes, post_logout_redirect_uris";

fn backend(err: sqlx::Error) -> RepositoryError {
    RepositoryError::Backend(err.to_string())
}

/// The module side of a row, `None` for a client registered by hand.
pub(crate) fn module_of(
    row: &sqlx::postgres::PgRow,
) -> Result<Option<ModuleClient>, RepositoryError> {
    let Some(module) = row
        .try_get::<Option<String>, _>("module")
        .map_err(backend)?
    else {
        return Ok(None);
    };
    let disabled: Option<DateTime<Utc>> = row.try_get("disabled_at").map_err(backend)?;
    let hash: Option<String> = row.try_get("previous_secret_hash").map_err(backend)?;
    let until: Option<DateTime<Utc>> = row.try_get("previous_secret_until").map_err(backend)?;
    let claim: Option<Value> = row.try_get("role_claim").map_err(backend)?;
    let consumes: Option<Vec<String>> = row.try_get("consumes").map_err(backend)?;
    let logout: Option<Vec<String>> = row.try_get("post_logout_redirect_uris").map_err(backend)?;
    Ok(Some(ModuleClient {
        module,
        disabled: disabled.is_some(),
        previous_secret: hash.zip(until).map(|(hash, until)| PreviousSecret {
            hash,
            until: SystemTime::from(until),
        }),
        role_claim: claim.as_ref().map(claim_of).transpose()?,
        consumes: consumes.unwrap_or_default(),
        logout_uris: logout.unwrap_or_default(),
    }))
}

fn claim_of(v: &Value) -> Result<RoleClaim, RepositoryError> {
    let bad = || RepositoryError::Backend("stored role_claim".into());
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
    let values = v.get("values").and_then(Value::as_array).ok_or_else(bad)?;
    Ok(RoleClaim {
        name: text(v, "name").ok_or_else(bad)?,
        values: values
            .iter()
            .map(|v| {
                Some(RoleValue {
                    value: text(v, "value")?,
                    action: text(v, "action")?,
                })
            })
            .collect::<Option<_>>()
            .ok_or_else(bad)?,
    })
}

fn claim_json(c: &RoleClaim) -> Value {
    let values: Vec<Value> = c
        .values
        .iter()
        .map(|v| json!({ "value": v.value, "action": v.action }))
        .collect();
    json!({ "name": c.name, "values": values })
}

/// Register or replace a module's client whole; `Ok(false)` when that id
/// is a client a person registered by hand, or another module's, which is
/// left alone.
pub(crate) async fn save(pool: &PgPool, client: &OidcClient) -> Result<bool, RepositoryError> {
    let Some(m) = &client.settings.module else {
        return Err(RepositoryError::Backend("not a module's client".into()));
    };
    let (secret_hash, None) = auth_columns(&client.auth) else {
        return Err(RepositoryError::Backend(
            "a module's client has no machine role".into(),
        ));
    };
    let s = &client.settings;
    let (previous_hash, previous_until) = match &m.previous_secret {
        Some(p) => (Some(p.hash.as_str()), Some(DateTime::<Utc>::from(p.until))),
        None => (None, None),
    };
    let saved = sqlx::query(
        "INSERT INTO oidc_clients (client_id, name, redirect_uris, secret_hash, token_alg, \
         pkce_required, grant_client_credentials, audiences, module, disabled_at, \
         previous_secret_hash, previous_secret_until, role_claim, consumes, \
         post_logout_redirect_uris) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, CASE WHEN $10 THEN now() END, \
                 $11, $12, $13, $14, $15) \
         ON CONFLICT (tenant_id, client_id) DO UPDATE SET \
         name = EXCLUDED.name, redirect_uris = EXCLUDED.redirect_uris, \
         secret_hash = EXCLUDED.secret_hash, token_alg = EXCLUDED.token_alg, \
         pkce_required = EXCLUDED.pkce_required, \
         grant_client_credentials = EXCLUDED.grant_client_credentials, \
         audiences = EXCLUDED.audiences, \
         disabled_at = CASE WHEN $10 THEN COALESCE(oidc_clients.disabled_at, now()) END, \
         previous_secret_hash = EXCLUDED.previous_secret_hash, \
         previous_secret_until = EXCLUDED.previous_secret_until, \
         role_claim = EXCLUDED.role_claim, consumes = EXCLUDED.consumes, \
         post_logout_redirect_uris = EXCLUDED.post_logout_redirect_uris \
         WHERE oidc_clients.module = EXCLUDED.module",
    )
    .bind(client.id.as_str())
    .bind(&client.name)
    .bind(&client.redirect_uris)
    .bind(secret_hash)
    .bind(s.token_alg.as_str())
    .bind(s.pkce_required)
    .bind(s.client_credentials)
    .bind(&s.audiences)
    .bind(&m.module)
    .bind(m.disabled)
    .bind(previous_hash)
    .bind(previous_until)
    .bind(m.role_claim.as_ref().map(claim_json))
    .bind(&m.consumes)
    .bind(&m.logout_uris)
    .execute(pool)
    .await
    .map_err(backend)?;
    Ok(saved.rows_affected() == 1)
}
