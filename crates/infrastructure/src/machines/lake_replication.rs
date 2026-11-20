//! What the replication left of each warehouse, in Postgres
//! (`lake_replication`, 0066 step 11): one row per warehouse, replaced at
//! each run, when it was first configured kept through later failures.

use std::time::SystemTime;

use async_trait::async_trait;
use auth_gateway::{
    Lake, ReplicationOutcome, ReplicationStateStore, ReplicationStep, WarehouseState,
};
use chrono::{DateTime, Utc};
use domain::RepositoryError;
use sqlx::postgres::{PgPool, PgRow};
use sqlx::Row;

use super::backend;

/// A Postgres-backed [`ReplicationStateStore`].
#[derive(Debug, Clone)]
pub struct PgReplicationState {
    pool: PgPool,
}

impl PgReplicationState {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const STEPS: [ReplicationStep; 9] = [
    ReplicationStep::WarehouseBucket,
    ReplicationStep::WarehouseTables,
    ReplicationStep::SourceBucket,
    ReplicationStep::ReplicaBucket,
    ReplicationStep::ReplicaRetention,
    ReplicationStep::SourceVersioning,
    ReplicationStep::SourceLifecycle,
    ReplicationStep::RemoteTarget,
    ReplicationStep::Rule,
];

/// As stored: the state, the step or lake, and the detail.
fn columns(outcome: &ReplicationOutcome) -> (&'static str, Option<String>, Option<String>) {
    match outcome {
        ReplicationOutcome::Configured => ("configured", None, None),
        ReplicationOutcome::NoWarehouse => ("no_warehouse", None, None),
        ReplicationOutcome::Made => ("made", None, None),
        ReplicationOutcome::NoKeys { lake } => ("no_keys", Some(lake.key()), None),
        ReplicationOutcome::Failed { step, detail } => {
            ("failed", Some(step.as_str().into()), Some(detail.clone()))
        }
    }
}

fn outcome(
    state: &str,
    step: Option<String>,
    detail: Option<String>,
) -> Result<ReplicationOutcome, RepositoryError> {
    let bad = || RepositoryError::Backend(format!("lake_replication: state {state} {step:?}"));
    Ok(match state {
        "configured" => ReplicationOutcome::Configured,
        "no_warehouse" => ReplicationOutcome::NoWarehouse,
        "made" => ReplicationOutcome::Made,
        "no_keys" => ReplicationOutcome::NoKeys {
            lake: step.as_deref().and_then(Lake::from_key).ok_or_else(bad)?,
        },
        "failed" => ReplicationOutcome::Failed {
            step: STEPS
                .into_iter()
                .find(|s| Some(s.as_str()) == step.as_deref())
                .ok_or_else(bad)?,
            detail: detail.unwrap_or_default(),
        },
        _ => return Err(bad()),
    })
}

const COLUMNS: &str = "warehouse, state, step, detail, configured_at, checked_at";

fn state(row: &PgRow) -> Result<WarehouseState, RepositoryError> {
    let configured: Option<DateTime<Utc>> = row.try_get("configured_at").map_err(backend)?;
    let checked: DateTime<Utc> = row.try_get("checked_at").map_err(backend)?;
    let kind: String = row.try_get("state").map_err(backend)?;
    Ok(WarehouseState {
        warehouse: row.try_get("warehouse").map_err(backend)?,
        outcome: outcome(
            &kind,
            row.try_get("step").map_err(backend)?,
            row.try_get("detail").map_err(backend)?,
        )?,
        configured_at: configured.map(SystemTime::from),
        checked_at: SystemTime::from(checked),
    })
}

#[async_trait]
impl ReplicationStateStore for PgReplicationState {
    async fn record(
        &self,
        warehouse: &str,
        outcome: &ReplicationOutcome,
        at: SystemTime,
    ) -> Result<WarehouseState, RepositoryError> {
        let (kind, step, detail) = columns(outcome);
        let at = DateTime::<Utc>::from(at);
        let configured = (kind == "configured").then_some(at);
        let row = sqlx::query(&format!(
            "INSERT INTO lake_replication (warehouse, state, step, detail, configured_at, checked_at) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (tenant_id, warehouse) DO UPDATE SET state = EXCLUDED.state, \
             step = EXCLUDED.step, detail = EXCLUDED.detail, checked_at = EXCLUDED.checked_at, \
             configured_at = COALESCE(lake_replication.configured_at, EXCLUDED.configured_at) \
             RETURNING {COLUMNS}"
        ))
        .bind(warehouse)
        .bind(kind)
        .bind(step)
        .bind(detail)
        .bind(configured)
        .bind(at)
        .fetch_one(&self.pool)
        .await
        .map_err(backend)?;
        state(&row)
    }

    async fn all(&self) -> Result<Vec<WarehouseState>, RepositoryError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM lake_replication ORDER BY warehouse"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        rows.iter().map(state).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_outcome_is_read_back_as_it_was_kept() {
        let all = [
            ReplicationOutcome::Configured,
            ReplicationOutcome::NoWarehouse,
            ReplicationOutcome::Made,
            ReplicationOutcome::NoKeys { lake: Lake::main() },
            ReplicationOutcome::NoKeys {
                lake: Lake::Replica,
            },
        ]
        .into_iter()
        .chain(STEPS.into_iter().map(|step| ReplicationOutcome::Failed {
            step,
            detail: "HTTP 403 AccessDenied".into(),
        }));
        for kept in all {
            let (kind, step, detail) = columns(&kept);
            assert_eq!(outcome(kind, step, detail).unwrap(), kept);
        }
        assert!(outcome("other", None, None).is_err());
        assert!(outcome("failed", Some("nope".into()), None).is_err());
    }
}
