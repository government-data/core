//! What a job is given besides its details (decisions 0030 and 0036): an
//! extraction's source, with both versions of its credentials, and every
//! credential a job gets recorded first in the audit log, for which job and
//! to which machine. A credential that cannot be recorded is not given.

use std::collections::BTreeMap;
use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use axum::http::{HeaderMap, StatusCode};

use crate::lake::{self, LakeAccess, LakeIssuer};
use crate::mapping::Mapping;
use crate::source::{driver, Engine};
use crate::source_catalog::has_catalog;
use crate::source_store::{self, Sealer, StoreError};

/// Where credentials issued to jobs are recorded (the audit log, 0026).
#[async_trait]
pub trait IssueLog: Send + Sync {
    /// `machine` (its client id, from `ip`) got credentials for `resource`
    /// (`source/core`, `warehouse/<bucket>`) for the `kind` job `job`.
    async fn issued(
        &self,
        job: Uuid,
        kind: &str,
        machine: &str,
        ip: &str,
        resource: &str,
    ) -> Result<(), String>;
}

/// An extraction's source as its job gets it: the driver, its fields, the
/// table the mapping reads (none for a listing), and its secrets, current
/// and previous. Kept in the worker's memory only; never printed.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceAccess {
    pub name: String,
    pub driver: String,
    pub table: String,
    pub fields: BTreeMap<String, String>,
    pub secrets: BTreeMap<String, String>,
    /// Tried only when the current ones are refused.
    pub previous: Option<BTreeMap<String, String>>,
}

impl SourceAccess {
    /// Read through its catalog on Trino: it carries no credentials.
    pub fn on_trino(&self) -> bool {
        has_catalog(&self.driver)
    }
}

impl fmt::Debug for SourceAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceAccess")
            .field("name", &self.name)
            .field("driver", &self.driver)
            .field("table", &self.table)
            .field("fields", &self.fields)
            .finish_non_exhaustive()
    }
}

/// Why a job could not get what it needs from here.
#[derive(Debug)]
pub enum SourceIssue {
    /// The sources' key or the audit log is not there or did not answer.
    Unavailable(String),
}

/// The source a job reads, if it names one that exists and that DuckDB
/// reads: an extraction's, from its dataset's mapping (`from`); a map's,
/// from its parameters (`source`, `table`: it has no mapping yet, decision
/// 0038); a page of records' too (0049); a listing's, from its parameters
/// too, with no table (0042). A
/// listing or an extraction of a source on Trino gets its name, driver and
/// table only: Trino reads its own catalog, and the worker never sees its
/// credentials (0048). Not yet recorded.
pub async fn source_for(
    pool: &PgPool,
    sealer: &dyn Sealer,
    kind: &str,
    parameters: &serde_json::Value,
    mapping: Option<&str>,
) -> Result<Option<SourceAccess>, SourceIssue> {
    let asked = |key: &str| {
        parameters
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let named = match kind {
        "extract" => mapping
            .and_then(|m| Mapping::parse(m).ok())
            .and_then(|m| m.from)
            .map(|f| (f.value.source, f.value.table)),
        "map" | "records" => asked("source").zip(asked("table")),
        "tables" => asked("source").map(|s| (s, String::new())),
        _ => None,
    };
    let Some((name, table)) = named else {
        return Ok(None);
    };
    let credentials = match source_store::credentials(pool, sealer, &name).await {
        Ok(c) => c,
        Err(StoreError::NotFound) => return Ok(None),
        Err(e) => return Err(SourceIssue::Unavailable(format!("{e:?}"))),
    };
    let on_duckdb = driver(&credentials.source.driver)
        .is_some_and(|d| matches!(d.engine, Engine::DuckDb { .. }));
    if !on_duckdb {
        let listed = matches!(kind, "tables" | "extract" | "records" | "map")
            && has_catalog(&credentials.source.driver);
        return Ok(listed.then(|| SourceAccess {
            name: credentials.source.name,
            driver: credentials.source.driver,
            table,
            fields: BTreeMap::new(),
            secrets: BTreeMap::new(),
            previous: None,
        }));
    }
    let source = credentials.source;
    Ok(Some(SourceAccess {
        name: source.name,
        driver: source.driver,
        table,
        fields: source.fields,
        secrets: source.secrets,
        previous: credentials.previous,
    }))
}

/// Read-only credentials for the tenant's warehouse, named after the job.
pub(crate) async fn read_access(
    issuer: &dyn LakeIssuer,
    tenant: Uuid,
    job: Uuid,
) -> Result<LakeAccess, StatusCode> {
    let warehouse = lake::warehouse(tenant);
    issuer
        .issue(
            &warehouse,
            &format!("job-{job}"),
            &lake::read_only_policy(&warehouse),
        )
        .await
        .map_err(|e| {
            tracing::error!("lake credentials: {e}");
            StatusCode::SERVICE_UNAVAILABLE
        })
}

/// Records that `client`'s worker gets credentials for `resource`;
/// without the log, or if it fails, they are not given.
pub(crate) async fn record(
    log: Option<&dyn IssueLog>,
    job: Uuid,
    kind: &str,
    client: &str,
    headers: &HeaderMap,
    resource: &str,
) -> Result<(), StatusCode> {
    // The worker's address as the proxy saw it.
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.split(',').next())
        .map_or("unknown", str::trim);
    let log = log.ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    log.issued(job, kind, client, ip, resource)
        .await
        .map_err(|e| {
            tracing::error!("recording credentials: {e}");
            StatusCode::SERVICE_UNAVAILABLE
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_access_never_prints_its_secrets() {
        let access = SourceAccess {
            name: "core".into(),
            driver: "postgres".into(),
            table: "public.clients".into(),
            fields: BTreeMap::new(),
            secrets: [("password".to_string(), "synthetic-secret".to_string())].into(),
            previous: Some([("password".to_string(), "older-secret".to_string())].into()),
        };
        let printed = format!("{access:?}");
        assert!(printed.contains("core"));
        assert!(!printed.contains("synthetic-secret") && !printed.contains("older-secret"));
    }
}
