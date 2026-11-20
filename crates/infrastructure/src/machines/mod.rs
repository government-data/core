//! The machines and their join tokens in Postgres (`machines`,
//! `join_tokens`, decisions 0029 and 0066), in the tenant the pool sets.
//! Using a token is one `UPDATE … WHERE used_at IS NULL AND expires_at > now`
//! in the same transaction that makes the machine: of two at once, one wins.

mod alerts;
mod install_files;
mod join_check;
mod lake_admin_keys;
mod lake_keys;
mod lake_pools;
mod lake_replication;
mod mint;
mod sessions;

use std::time::SystemTime;

use async_trait::async_trait;
use auth_gateway::{
    Machine, MachineChange, MachineId, MachineRegistry, MachineRole, MachineState, NewJoinToken,
    RedeemRefusal,
};
use chrono::{DateTime, Utc};
use domain::{AdminId, RepositoryError};
use sqlx::postgres::{PgPool, PgRow};
use sqlx::Row;
use uuid::Uuid;

pub use alerts::PgMachineAlerts;
pub use install_files::{
    sha256_hex, EmbeddedInstallFiles, JOIN_SCRIPT, LAKE_ALLOY, LAKE_GATE_ENTRYPOINT,
    LAKE_POOL_COMPOSE, LAKE_PROXY_ENTRYPOINT, LAKE_REPLICA_ALLOY, LAKE_REPLICA_COMPOSE,
    RUSTFS_TLS_ENTRYPOINT, WORKER_ALLOY, WORKER_COMPOSE,
};
pub use lake_admin_keys::PgLakeAdminKeys;
pub use lake_keys::PgLakeRootKeys;
pub use lake_pools::PgLakePools;
pub use lake_replication::PgReplicationState;
pub use mint::RingJoinTokenMint;
pub use sessions::PgMachineSessions;

const COLUMNS: &str = "id::text AS id, name, role, pool, state, version, cert_serial, \
     cert_not_after, cert_serial_next, cert_not_after_next, created_at, approved_at, approved_by::text AS approved_by, retired_at, \
     retired_by::text AS retired_by";

/// A Postgres-backed [`MachineRegistry`].
#[derive(Debug, Clone)]
pub struct PgMachines {
    pool: PgPool,
}

impl PgMachines {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The machine an `UPDATE … RETURNING` changed, or why it changed none.
    async fn changed(
        &self,
        id: Uuid,
        row: Option<PgRow>,
    ) -> Result<MachineChange, RepositoryError> {
        match row {
            Some(row) => Ok(MachineChange::Done(Box::new(machine(&row)?))),
            None => self.state_of(id).await,
        }
    }

    /// The state of machine `id`, if it exists.
    async fn state_of(&self, id: Uuid) -> Result<MachineChange, RepositoryError> {
        let state: Option<String> = sqlx::query_scalar("SELECT state FROM machines WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?;
        Ok(match state {
            Some(state) => MachineChange::NotAllowed(parse_state(&state)?),
            None => MachineChange::NotFound,
        })
    }
}

fn backend(e: sqlx::Error) -> RepositoryError {
    RepositoryError::Backend(e.to_string())
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .and_then(|d| d.code())
        .is_some_and(|c| c == "23505")
}

fn parse_state(raw: &str) -> Result<MachineState, RepositoryError> {
    MachineState::parse(raw).ok_or_else(|| RepositoryError::Backend(format!("state {raw:?}")))
}

fn time(at: Option<DateTime<Utc>>) -> Option<SystemTime> {
    at.map(SystemTime::from)
}

fn machine(row: &PgRow) -> Result<Machine, RepositoryError> {
    let get = |c: &str| row.try_get::<Option<String>, _>(c).map_err(backend);
    let at = |c: &str| row.try_get::<Option<DateTime<Utc>>, _>(c).map_err(backend);
    let role: String = row.try_get("role").map_err(backend)?;
    let state: String = row.try_get("state").map_err(backend)?;
    let created_at: DateTime<Utc> = row.try_get("created_at").map_err(backend)?;
    Ok(Machine {
        id: MachineId::new(row.try_get::<String, _>("id").map_err(backend)?),
        name: row.try_get("name").map_err(backend)?,
        role: MachineRole::parse(&role)
            .ok_or_else(|| RepositoryError::Backend(format!("role {role:?}")))?,
        pool: get("pool")?,
        state: parse_state(&state)?,
        version: get("version")?,
        cert_serial: get("cert_serial")?,
        cert_not_after: time(at("cert_not_after")?),
        cert_serial_next: get("cert_serial_next")?,
        cert_not_after_next: time(at("cert_not_after_next")?),
        created_at: created_at.into(),
        approved_at: time(at("approved_at")?),
        approved_by: get("approved_by")?.map(AdminId::new),
        retired_at: time(at("retired_at")?),
        retired_by: get("retired_by")?.map(AdminId::new),
    })
}

#[async_trait]
impl MachineRegistry for PgMachines {
    async fn issue_token(&self, token: &NewJoinToken) -> Result<String, RepositoryError> {
        sqlx::query_scalar(
            "INSERT INTO join_tokens (token_hash, role, pool, node, created_by, expires_at) \
             VALUES ($1, $2, $3, $6, $4::uuid, $5) RETURNING id::text",
        )
        .bind(&token.token_hash)
        .bind(token.role.as_str())
        .bind(&token.pool)
        .bind(token.created_by.as_str())
        .bind(DateTime::<Utc>::from(token.expires_at))
        .bind(&token.node)
        .fetch_one(&self.pool)
        .await
        .map_err(backend)
    }

    async fn redeem(
        &self,
        token_hash: &str,
        name: &str,
        now: SystemTime,
    ) -> Result<Result<Machine, RedeemRefusal>, RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(backend)?;
        let used = sqlx::query(
            "UPDATE join_tokens SET used_at = $2 \
             WHERE token_hash = $1 AND used_at IS NULL AND expires_at > $2 \
             RETURNING id, role, pool, node",
        )
        .bind(token_hash)
        .bind(DateTime::<Utc>::from(now))
        .fetch_optional(&mut *tx)
        .await
        .map_err(backend)?;
        let Some(used) = used else {
            return Ok(Err(RedeemRefusal::Token));
        };
        let token: Uuid = used.try_get("id").map_err(backend)?;
        let role: String = used.try_get("role").map_err(backend)?;
        let pool: Option<String> = used.try_get("pool").map_err(backend)?;
        let node: Option<String> = used.try_get("node").map_err(backend)?;
        if node.is_some_and(|node| node != name) {
            // Dropping the transaction rolls it back: the token stays unused.
            return Ok(Err(RedeemRefusal::NodeName));
        }
        let made = sqlx::query(&format!(
            "INSERT INTO machines (name, role, pool) VALUES ($1, $2, $3) RETURNING {COLUMNS}"
        ))
        .bind(name)
        .bind(&role)
        .bind(&pool)
        .fetch_one(&mut *tx)
        .await;
        let made = match made {
            Ok(row) => machine(&row)?,
            // Dropping the transaction rolls it back: the token stays unused.
            Err(e) if is_unique_violation(&e) => return Ok(Err(RedeemRefusal::NameTaken)),
            Err(e) => return Err(backend(e)),
        };
        sqlx::query("UPDATE join_tokens SET machine_id = $1::uuid WHERE id = $2")
            .bind(made.id.as_str())
            .bind(token)
            .execute(&mut *tx)
            .await
            .map_err(backend)?;
        tx.commit().await.map_err(backend)?;
        Ok(Ok(made))
    }

    async fn create_approved(
        &self,
        name: &str,
        role: MachineRole,
        now: SystemTime,
    ) -> Result<Option<Machine>, RepositoryError> {
        let made = sqlx::query(&format!(
            "INSERT INTO machines (name, role, state, approved_at) \
             VALUES ($1, $2, 'approved', $3) RETURNING {COLUMNS}"
        ))
        .bind(name)
        .bind(role.as_str())
        .bind(DateTime::<Utc>::from(now))
        .fetch_one(&self.pool)
        .await;
        match made {
            Ok(row) => machine(&row).map(Some),
            Err(e) if is_unique_violation(&e) => Ok(None),
            Err(e) => Err(backend(e)),
        }
    }

    async fn transition(
        &self,
        id: &MachineId,
        to: MachineState,
        by: &AdminId,
        now: SystemTime,
    ) -> Result<MachineChange, RepositoryError> {
        let Ok(id) = Uuid::parse_str(id.as_str()) else {
            return Ok(MachineChange::NotFound);
        };
        let from: Vec<&str> = to.reached_from().iter().map(|s| s.as_str()).collect();
        let row = sqlx::query(&format!(
            "UPDATE machines SET state = $2, \
               approved_at = CASE WHEN $2 = 'approved' THEN $4 ELSE approved_at END, \
               approved_by = CASE WHEN $2 = 'approved' THEN $3::uuid ELSE approved_by END, \
               retired_at = CASE WHEN $2 = 'retired' THEN $4 ELSE retired_at END, \
               retired_by = CASE WHEN $2 = 'retired' THEN $3::uuid ELSE retired_by END \
             WHERE id = $1 AND state = ANY($5) RETURNING {COLUMNS}"
        ))
        .bind(id)
        .bind(to.as_str())
        .bind(by.as_str())
        .bind(DateTime::<Utc>::from(now))
        .bind(&from)
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?;
        match row {
            Some(row) => Ok(MachineChange::Done(Box::new(machine(&row)?))),
            None => self.state_of(id).await,
        }
    }

    async fn record_certificate(
        &self,
        id: &MachineId,
        serial: &str,
        not_after: SystemTime,
    ) -> Result<MachineChange, RepositoryError> {
        let Ok(id) = Uuid::parse_str(id.as_str()) else {
            return Ok(MachineChange::NotFound);
        };
        let row = sqlx::query(&format!(
            "UPDATE machines SET cert_serial = $2, cert_not_after = $3, \
               cert_serial_next = NULL, cert_not_after_next = NULL \
             WHERE id = $1 AND state <> 'retired' RETURNING {COLUMNS}"
        ))
        .bind(id)
        .bind(serial)
        .bind(DateTime::<Utc>::from(not_after))
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?;
        self.changed(id, row).await
    }

    async fn record_next_certificate(
        &self,
        id: &MachineId,
        serial: &str,
        not_after: SystemTime,
    ) -> Result<MachineChange, RepositoryError> {
        let Ok(id) = Uuid::parse_str(id.as_str()) else {
            return Ok(MachineChange::NotFound);
        };
        let row = sqlx::query(&format!(
            "UPDATE machines SET cert_serial_next = $2, cert_not_after_next = $3 \
             WHERE id = $1 AND state <> 'retired' RETURNING {COLUMNS}"
        ))
        .bind(id)
        .bind(serial)
        .bind(DateTime::<Utc>::from(not_after))
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?;
        self.changed(id, row).await
    }

    async fn promote_next_certificate(
        &self,
        id: &MachineId,
        serial: &str,
    ) -> Result<MachineChange, RepositoryError> {
        let Ok(id) = Uuid::parse_str(id.as_str()) else {
            return Ok(MachineChange::NotFound);
        };
        let row = sqlx::query(&format!(
            "UPDATE machines SET cert_serial = cert_serial_next, \
               cert_not_after = cert_not_after_next, \
               cert_serial_next = NULL, cert_not_after_next = NULL \
             WHERE id = $1 AND cert_serial_next = $2 AND state <> 'retired' \
             RETURNING {COLUMNS}"
        ))
        .bind(id)
        .bind(serial)
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?;
        self.changed(id, row).await
    }

    async fn find(&self, id: &MachineId) -> Result<Option<Machine>, RepositoryError> {
        let Ok(id) = Uuid::parse_str(id.as_str()) else {
            return Ok(None);
        };
        let sql = format!("SELECT {COLUMNS} FROM machines WHERE id = $1");
        let row = sqlx::query(&sql).bind(id).fetch_optional(&self.pool).await;
        row.map_err(backend)?.as_ref().map(machine).transpose()
    }

    async fn find_live(&self, name: &str) -> Result<Option<Machine>, RepositoryError> {
        let sql = format!("SELECT {COLUMNS} FROM machines WHERE name = $1 AND state <> 'retired'");
        let row = sqlx::query(&sql)
            .bind(name)
            .fetch_optional(&self.pool)
            .await;
        row.map_err(backend)?.as_ref().map(machine).transpose()
    }

    async fn list(&self) -> Result<Vec<Machine>, RepositoryError> {
        let sql = format!("SELECT {COLUMNS} FROM machines ORDER BY name, created_at");
        let rows = sqlx::query(&sql).fetch_all(&self.pool).await;
        rows.map_err(backend)?.iter().map(machine).collect()
    }
}
