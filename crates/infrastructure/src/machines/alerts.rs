//! The machines' active alerts in Postgres (`machine_alerts`, decision
//! 0072 step 5), in the tenant the pool sets: a delivery's changes in one
//! transaction. The same alert again updates nothing (the upsert's `WHERE …
//! IS DISTINCT FROM`); the heartbeat's row takes every delivery's time.

use std::time::SystemTime;

use async_trait::async_trait;
use auth_gateway::{AlertChange, AlertKey, FiringAlert, MachineAlertStore, ALERTING_HEARTBEAT};
use chrono::{DateTime, Utc};
use domain::RepositoryError;
use sqlx::postgres::PgPool;
use sqlx::types::Json;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::backend;

/// A Postgres-backed [`MachineAlertStore`].
#[derive(Debug, Clone)]
pub struct PgMachineAlerts {
    pool: PgPool,
}

impl PgMachineAlerts {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// What a kept row may change to; equal, nothing is written.
const CHANGED: &str = "SET fingerprint = EXCLUDED.fingerprint, qd_alert = EXCLUDED.qd_alert, \
     severity = EXCLUDED.severity, starts_at = EXCLUDED.starts_at, \
     \"values\" = EXCLUDED.\"values\", received_at = EXCLUDED.received_at \
     WHERE (machine_alerts.fingerprint, machine_alerts.qd_alert, machine_alerts.severity, \
            machine_alerts.starts_at, machine_alerts.\"values\") \
       IS DISTINCT FROM (EXCLUDED.fingerprint, EXCLUDED.qd_alert, EXCLUDED.severity, \
            EXCLUDED.starts_at, EXCLUDED.\"values\")";

const MACHINELESS: &str = "ON CONFLICT (tenant_id, alertname, rule_name) WHERE machine_id IS NULL";

fn machine_id(machine: &auth_gateway::MachineId) -> Result<Uuid, RepositoryError> {
    Uuid::parse_str(machine.as_str())
        .map_err(|_| RepositoryError::Backend(format!("machine id {:?}", machine.as_str())))
}

async fn firing(
    tx: &mut Transaction<'_, Postgres>,
    alert: &FiringAlert,
    at: DateTime<Utc>,
) -> Result<(), RepositoryError> {
    let (machine, alertname, rule_name, conflict) = match &alert.key {
        AlertKey::Machine { machine, .. } => (
            Some(machine_id(machine)?),
            None,
            None,
            "ON CONFLICT ON CONSTRAINT machine_alerts_fingerprint",
        ),
        AlertKey::Machineless {
            alertname,
            rule_name,
        } => (
            None,
            Some(alertname.as_str()),
            Some(rule_name.as_str()),
            MACHINELESS,
        ),
    };
    sqlx::query(&format!(
        "INSERT INTO machine_alerts (machine_id, alertname, rule_name, fingerprint, qd_alert, \
           severity, starts_at, received_at, \"values\") \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) {conflict} DO UPDATE {CHANGED}"
    ))
    .bind(machine)
    .bind(alertname)
    .bind(rule_name)
    .bind(&alert.fingerprint)
    .bind(&alert.qd_alert)
    .bind(alert.severity.map(|s| s.as_str()))
    .bind(DateTime::<Utc>::from(alert.starts_at))
    .bind(at)
    .bind(Json(&alert.values))
    .execute(&mut **tx)
    .await
    .map_err(backend)?;
    Ok(())
}

async fn resolved(
    tx: &mut Transaction<'_, Postgres>,
    key: &AlertKey,
) -> Result<(), RepositoryError> {
    let deleted = match key {
        AlertKey::Machine {
            machine,
            fingerprint,
        } => sqlx::query("DELETE FROM machine_alerts WHERE machine_id = $1 AND fingerprint = $2")
            .bind(machine_id(machine)?)
            .bind(fingerprint),
        AlertKey::Machineless {
            alertname,
            rule_name,
        } => sqlx::query(
            "DELETE FROM machine_alerts \
             WHERE machine_id IS NULL AND alertname = $1 AND rule_name = $2",
        )
        .bind(alertname)
        .bind(rule_name),
    };
    deleted.execute(&mut **tx).await.map_err(backend)?;
    Ok(())
}

async fn heartbeat(
    tx: &mut Transaction<'_, Postgres>,
    fingerprint: &str,
    starts_at: SystemTime,
    at: DateTime<Utc>,
) -> Result<(), RepositoryError> {
    sqlx::query(&format!(
        "INSERT INTO machine_alerts (alertname, rule_name, fingerprint, qd_alert, starts_at, \
           received_at) \
         VALUES ($1, $1, $2, $1, $3, $4) {MACHINELESS} DO UPDATE \
         SET fingerprint = EXCLUDED.fingerprint, starts_at = EXCLUDED.starts_at, \
           received_at = GREATEST(machine_alerts.received_at, EXCLUDED.received_at)"
    ))
    .bind(ALERTING_HEARTBEAT)
    .bind(fingerprint)
    .bind(DateTime::<Utc>::from(starts_at))
    .bind(at)
    .execute(&mut **tx)
    .await
    .map_err(backend)?;
    Ok(())
}

#[async_trait]
impl MachineAlertStore for PgMachineAlerts {
    async fn apply(&self, changes: &[AlertChange], at: SystemTime) -> Result<(), RepositoryError> {
        let at = DateTime::<Utc>::from(at);
        let mut tx = self.pool.begin().await.map_err(backend)?;
        for change in changes {
            match change {
                AlertChange::Firing(alert) => firing(&mut tx, alert, at).await?,
                AlertChange::Resolved(key) => resolved(&mut tx, key).await?,
                AlertChange::Heartbeat {
                    fingerprint,
                    starts_at,
                } => heartbeat(&mut tx, fingerprint, *starts_at, at).await?,
            }
        }
        tx.commit().await.map_err(backend)
    }
}
