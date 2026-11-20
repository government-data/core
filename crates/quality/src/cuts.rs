//! A cut's chain (decision 0035, point 3): its extraction's report publishes
//! it and queues its evaluation in one transaction, with the bronze snapshot
//! just written; its evaluation's report marks it done. An evaluation never
//! reads a cut that is not published, and a repeated report changes nothing.

use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::firing::queue_job;

/// An `extract` job's report: names, types and counts, never a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractReport {
    /// `YYYY-MM-DD`: the cut's date.
    pub cut: String,
    pub duration_ms: i64,
    /// The source's table, as the mapping names it.
    pub source: String,
    /// The source's columns as the extraction saw them.
    pub schema: Vec<SourceColumn>,
    /// Each range read (a key range or a partition) and its rows.
    pub ranges: Vec<RangeRead>,
    /// The bronze snapshot it committed, by Iceberg id (text, as
    /// [`crate::worker_api::SnapshotRead`]).
    pub snapshot: String,
    /// The silver snapshot it committed (decision 0061); none from an older
    /// worker, a Trino extraction or a mapping silver could not take.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub silver_snapshot: Option<String>,
}

/// A source column: its name and its type as the source declares it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: String,
}

/// One range of an extraction and the rows it read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeRead {
    pub range: String,
    pub rows: i64,
}

impl ExtractReport {
    /// A report that can be true: a snapshot id, a duration and rows that
    /// are not negative.
    pub fn is_valid(&self) -> bool {
        self.snapshot.parse::<i64>().is_ok()
            && self
                .silver_snapshot
                .as_ref()
                .is_none_or(|s| s.parse::<i64>().is_ok())
            && self.duration_ms >= 0
            && self.ranges.iter().all(|r| r.rows >= 0)
    }
}

/// What an extraction's report did to its cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Published {
    /// The cut is published and its evaluation queued.
    Evaluating { cut: Uuid, evaluate: Uuid },
    /// Nothing to do: the job opened no cut, or its cut was published by a
    /// report before this one.
    Unchanged,
    /// The report is of another date than the job's cut.
    WrongCut,
}

/// Publishes the cut `extract_job` was queued for and queues its
/// evaluation, at `report.snapshot`, in one transaction.
pub async fn publish(
    pool: &PgPool,
    extract_job: Uuid,
    report: &ExtractReport,
) -> Result<Published, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let cut: Option<(Uuid, String, String, String, i32)> = sqlx::query_as(
        "SELECT id, dataset, cut::text, state, run FROM quality.cuts
          WHERE extract_job = $1 FOR UPDATE",
    )
    .bind(extract_job)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((cut, dataset, date, state, run)) = cut else {
        return Ok(Published::Unchanged);
    };
    if date != report.cut {
        return Ok(Published::WrongCut);
    }
    if state != "extracting" {
        return Ok(Published::Unchanged);
    }
    let mut parameters = serde_json::json!({
        "cut": date,
        "cut_id": cut,
        "run": run,
        "snapshot": report.snapshot,
    });
    if let Some(silver) = &report.silver_snapshot {
        parameters["silver_snapshot"] = silver.clone().into();
    }
    let evaluate = queue_job(&mut tx, "evaluate", Some(&dataset), &parameters).await?;
    sqlx::query(
        "UPDATE quality.cuts SET state = 'evaluating', bronze_snapshot = $2, evaluate_job = $3
          WHERE id = $1",
    )
    .bind(cut)
    .bind(&report.snapshot)
    .bind(evaluate)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Published::Evaluating { cut, evaluate })
}

/// Marks done the cut `evaluate_job` evaluates, if it is one's, in the
/// transaction that stores its results.
pub async fn finish_evaluation(
    tx: &mut Transaction<'_, Postgres>,
    evaluate_job: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE quality.cuts SET state = 'done'
          WHERE evaluate_job = $1 AND state = 'evaluating'",
    )
    .bind(evaluate_job)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
