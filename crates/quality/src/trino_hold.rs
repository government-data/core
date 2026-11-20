//! Holding the jobs on Trino while its coordinator restarts (decision 0066,
//! point 9), in the current tenant: the queue hands out no queued one while
//! the hold is in force (`claim_job`, migrations/0026_jobs_uses_trino.sql),
//! and one already running goes on. Which jobs is per job (the user's ruling
//! of 2026-11-11): `quality.jobs.uses_trino`, and when it is not known
//! (NULL), every job of a kind that may read through Trino. The server
//! holds and releases every tenant of its cell, for the machines' routes,
//! which ask it through a port.

use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

use crate::source_catalog::has_catalog;

/// The lock a claim takes shared and a hold alone (0066, point 9), so
/// that a claim is either counted or sees the hold.
const HOLD_LOCK: i64 = 6_600_009;

/// Holds the current tenant's jobs on Trino for `machine`, for `ttl` from
/// now (a hold in force is extended), and counts those running: a job known
/// not to use Trino is not counted, one not known is.
/// In one transaction, with the claims' lock taken alone: a claim that
/// began before has committed by the count, one after sees the hold.
pub async fn hold(pool: &PgPool, machine: &str, ttl: Duration) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(HOLD_LOCK)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO quality.trino_holds (machine, expires_at)
         VALUES ($1, now() + make_interval(secs => $2))
         ON CONFLICT (tenant_id) DO UPDATE
            SET machine = EXCLUDED.machine, held_at = now(), expires_at = EXCLUDED.expires_at",
    )
    .bind(machine)
    .bind(ttl.as_secs_f64())
    .execute(&mut *tx)
    .await?;
    let running: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM quality.jobs
          WHERE state = 'running' AND quality.on_trino(kind, uses_trino)",
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(u64::try_from(running).unwrap_or_default())
}

/// Ends the current tenant's hold: whether one was in force (`false` when
/// there was none, or it had expired by itself).
pub async fn release(pool: &PgPool) -> Result<bool, sqlx::Error> {
    let in_force: Option<bool> =
        sqlx::query_scalar("DELETE FROM quality.trino_holds RETURNING expires_at > now()")
            .fetch_optional(pool)
            .await?;
    Ok(in_force.unwrap_or(false))
}

/// What a running job's details found (0066, point 9): whether it `uses`
/// Trino, the source it reads known now. Kept on the job, so the count
/// sees it from then on: a job marked otherwise when queued (its mapping
/// changed since, or not known) is corrected, either way. If it uses
/// Trino and its tenant's hold is in force (it was taken as known not to),
/// it goes back to the queue, its attempt not counted, and is held there:
/// `false`, it must not run now. With the claims' lock taken shared, as a
/// claim: a hold taken before sees it marked, one after is seen here. A
/// kind that never reads through Trino is left as it is. Only the run of
/// `worker`'s (the caller's prefix, as the details' route found it) goes
/// back: had its lease ended since and another taken the job, that run is
/// not this caller's to end.
pub async fn at_details(
    pool: &PgPool,
    job: Uuid,
    worker: &str,
    uses: bool,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock_shared($1)")
        .bind(HOLD_LOCK)
        .execute(&mut *tx)
        .await?;
    let held: Option<bool> = sqlx::query_scalar(
        "UPDATE quality.jobs SET uses_trino = $2
          WHERE id = $1 AND quality.may_use_trino(kind)
         RETURNING $2 AND EXISTS (
             SELECT 1 FROM quality.trino_holds h
              WHERE h.tenant_id = quality.jobs.tenant_id AND h.expires_at > now())",
    )
    .bind(job)
    .bind(uses)
    .fetch_optional(&mut *tx)
    .await?;
    let held = held.unwrap_or(false);
    if held {
        sqlx::query(
            "UPDATE quality.jobs
                SET state = 'queued', worker = NULL, lease_until = NULL, started_at = NULL,
                    attempts = greatest(attempts - 1, 0)
              WHERE id = $1 AND state = 'running'
                AND lease_until > now() AND starts_with(worker, $2 || '/')",
        )
        .bind(job)
        .bind(worker)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(!held)
}

/// Whether a `kind` job with `parameters` uses Trino, as its enqueuer
/// knows it (0066, point 9): a job on a source (`tables`, `map`,
/// `records`) by its source's driver, an extraction by the source its
/// dataset's mapping names, `extracted_from`, if the enqueuer read it. As
/// its details will say ([`crate::job_sources::SourceAccess::on_trino`]):
/// the driver has a catalog on Trino. `None` (held while Trino is) when
/// not known: no source named, or none of that name; and for a kind that
/// never reads through Trino, which no hold looks at.
pub(crate) async fn known_use<'c>(
    db: impl sqlx::PgExecutor<'c>,
    kind: &str,
    parameters: &serde_json::Value,
    extracted_from: Option<&str>,
) -> Result<Option<bool>, sqlx::Error> {
    let source = match kind {
        "tables" | "map" | "records" => parameters.get("source").and_then(|s| s.as_str()),
        "extract" => extracted_from,
        _ => None,
    };
    let Some(source) = source else {
        return Ok(None);
    };
    let driver: Option<String> =
        sqlx::query_scalar("SELECT driver FROM quality.sources WHERE name = $1")
            .bind(source)
            .fetch_optional(db)
            .await?;
    Ok(driver.map(|d| has_catalog(&d)))
}
