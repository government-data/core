//! A job's token for Trino (decision 0062): a job that reads a source on
//! Trino (`tables`, `map`, `records`, `extract`) gets a JWT of its own,
//! for 15 minutes, whose subject names the tenant, the job and the
//! source's catalog, or an extraction's dataset; Trino maps it to a user
//! its rules let read that catalog only, and an extraction's write its
//! dataset's bronze schema too. It is recorded first, as every credential a job gets
//! (0030 point 3), and asked again on `GET /internal/jobs/{tenant}/{id}/trino`
//! when a statement would start with less than five minutes left.

use std::fmt;

use async_trait::async_trait;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::job_sources::{self, SourceAccess};
use crate::mapping::Mapping;
use crate::source::valid_name;
use crate::trino_catalog::catalog_name;
use crate::trino_rules::bronze_schema;
use crate::worker_api::{job_tenant, WorkerApi};

/// A job's token for Trino, as its details carry it. Kept in the worker's
/// memory only; never printed.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrinoAccess {
    pub token: String,
    /// Seconds it lasts from when it was given.
    pub expires_in: u64,
}

impl fmt::Debug for TrinoAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrinoAccess")
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

/// Port: signs a job's token (the server: the installation's OIDC
/// provider, as the API's own machine client).
#[async_trait]
pub trait TrinoJobTokens: Send + Sync {
    /// A token for `subject`, given to the machine `actor`.
    async fn issue(&self, subject: &str, actor: &str) -> Result<TrinoAccess, String>;
}

/// The subject of a `kind` job's token reading `source`:
/// `job:<tenant>:<job>:read:src_<source>`, or `…:extract:<dataset>` for an
/// extraction, whose user Trino's rules tie to its dataset's source and
/// bronze schema ([`crate::trino_rules`]). `None` for a kind that does not
/// read through Trino, and for an extraction with no dataset or one whose
/// name is no user's (`a-z`, `0-9`, `_`, starting with a letter).
pub fn subject(
    tenant: Uuid,
    job: Uuid,
    kind: &str,
    source: &str,
    dataset: Option<&str>,
) -> Option<String> {
    let scope = match kind {
        "extract" => format!("extract:{}", dataset.filter(|d| valid_name(d))?),
        "tables" | "map" | "records" => format!("read:{}", catalog_name(source)),
        _ => return None,
    };
    Some(format!("job:{tenant}:{job}:{scope}"))
}

/// What the audit log records as given: `trino/src_<source>`, with
/// `+<dataset>_bronze` for an extraction.
pub fn resource(source: &str, dataset: Option<&str>) -> String {
    let bronze = dataset.map_or(String::new(), |d| format!("+{}", bronze_schema(d)));
    format!("trino/{}{bronze}", catalog_name(source))
}

/// The dataset an extraction writes, as its `mapping` names it (its bronze
/// schema, [`crate::trino_extract`]); `None` for any other kind.
pub(crate) fn extracted(kind: &str, mapping: Option<&str>) -> Option<String> {
    let mapping = mapping.filter(|_| kind == "extract")?;
    Mapping::parse(mapping).ok().map(|m| m.dataset.value)
}

/// The token of a `kind` job reading `source` (an extraction, into
/// `dataset`), recorded first; `None` if the source is not on Trino or the
/// kind does not read through it. Without the issuer, or if it fails, the
/// job gets nothing (503).
pub(crate) async fn access(
    api: &WorkerApi,
    (tenant, job): (Uuid, Uuid),
    kind: &str,
    client: &str,
    headers: &HeaderMap,
    source: &SourceAccess,
    dataset: Option<&str>,
) -> Result<Option<TrinoAccess>, StatusCode> {
    let Some(subject) =
        subject(tenant, job, kind, &source.name, dataset).filter(|_| source.on_trino())
    else {
        return Ok(None);
    };
    let Some(tokens) = &api.services.trino_jobs else {
        tracing::error!("a job on Trino, and no issuer of its token");
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };
    let log = api.services.issues.as_deref();
    let given = resource(&source.name, dataset);
    job_sources::record(log, job, kind, client, headers, &given).await?;
    tokens.issue(&subject, client).await.map(Some).map_err(|e| {
        tracing::error!("trino token: {e}");
        StatusCode::SERVICE_UNAVAILABLE
    })
}

/// `GET /internal/jobs/{tenant}/{id}/trino`: a job on Trino asks again for
/// its token. 404 if the job is not the caller's or reads no source on
/// Trino.
pub(crate) async fn again(
    State(api): State<WorkerApi>,
    Path((tenant, id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<TrinoAccess>, StatusCode> {
    let client = api.caller(&headers)?;
    tenancy::within(job_tenant(tenant), async {
        let job = api.held(tenant, id, &client).await?;
        // An extraction's source is its dataset's mapping's `from`.
        let snapshot = match &job.dataset {
            Some(dataset) if job.kind == "extract" => api.snapshot(tenant, dataset).await?,
            _ => None,
        };
        let mapping = snapshot.as_ref().map(|s| s.mapping.as_str());
        let source = api
            .job_source(&job.kind, mapping, &job.parameters)
            .await?
            .ok_or(StatusCode::NOT_FOUND)?;
        let dataset = extracted(&job.kind, mapping);
        let ids = (tenant, id);
        access(
            &api,
            ids,
            &job.kind,
            &client,
            &headers,
            &source,
            dataset.as_deref(),
        )
        .await?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "00000000-0000-0000-0000-00000000000a";
    const J: &str = "7f0c2a52-1c1e-4c43-9a55-0d2f1e0b6a10";

    #[test]
    fn a_subject_names_the_tenant_the_job_and_the_catalog() {
        let (t, j) = (T.parse().unwrap(), J.parse().unwrap());
        assert_eq!(
            subject(t, j, "tables", "core", None).as_deref(),
            Some(format!("job:{T}:{J}:read:src_core").as_str())
        );
        for kind in ["map", "records"] {
            // A read job's subject names no dataset, whatever comes.
            assert!(subject(t, j, kind, "core", Some("clients"))
                .unwrap()
                .ends_with(":read:src_core"));
        }
        for kind in ["evaluate", "query", "test_rule", "maintain"] {
            assert_eq!(subject(t, j, kind, "core", Some("clients")), None);
        }
    }

    #[test]
    fn an_extractions_subject_names_its_dataset_not_its_source() {
        let (t, j) = (T.parse().unwrap(), J.parse().unwrap());
        assert_eq!(
            subject(t, j, "extract", "core", Some("clients")).as_deref(),
            Some(format!("job:{T}:{J}:extract:clients").as_str())
        );
        // Without a dataset, or with a name that is no user's, no token:
        // none that Trino would map to another's user.
        for dataset in [
            None,
            Some(""),
            Some("Clients"),
            Some("a:b"),
            Some("src_x|y"),
        ] {
            assert_eq!(subject(t, j, "extract", "core", dataset), None);
        }
    }

    #[test]
    fn an_extraction_is_recorded_with_its_bronze_schema() {
        assert_eq!(resource("core", None), "trino/src_core");
        assert_eq!(
            resource("core", Some("clients")),
            "trino/src_core+clients_bronze"
        );
    }

    #[test]
    fn only_an_extraction_has_a_dataset_and_it_is_its_mappings() {
        let mapping = "dataset: clients\nsource: c\nfields:\n  Id: { column: id, type: text }\n";
        assert_eq!(
            extracted("extract", Some(mapping)).as_deref(),
            Some("clients")
        );
        assert_eq!(extracted("map", Some(mapping)), None);
        assert_eq!(extracted("extract", None), None);
        assert_eq!(extracted("extract", Some("dataset: [")), None);
    }

    #[test]
    fn a_token_is_never_printed() {
        let access = TrinoAccess {
            token: "synthetic.jwt.value".into(),
            expires_in: 900,
        };
        let printed = format!("{access:?}");
        assert!(printed.contains("900") && !printed.contains("synthetic"));
    }
}
