//! The routes workers call (decisions 0018 and 0023): a job's details and
//! its report. The tenant comes from the job, never from the caller: the
//! route names the job by `(tenant, id)`, and the job must be running,
//! with a live lease, held by a worker of the machine whose token comes
//! (workers are named `<client id>/<slot>`, or `<client id>/<machine>/<slot>`
//! when the client is a role's and the token names the machine, decision
//! 0066 step 7). A job that does not match answers 404, whatever the
//! reason.

use std::sync::Arc;

use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::catalog::{Catalog, CatalogSnapshot};
use crate::entity_key::{self, EntityKey};
use crate::job_lake;
use crate::job_maintain;
use crate::job_map::Drafts;
use crate::job_sources::{self, IssueLog, SourceAccess};
use crate::job_span;
use crate::lake::{LakeAccess, LakeIssuer, TableCatalog};
use crate::lake_tables;
use crate::manifest::ManifestStore;
use crate::proposal::Alerts;
use crate::records_api;
pub use crate::report::{
    band, cell, CellState, CompiledRule, Distribution, EntityIcdRow, FieldQuality, GroupMetrics,
    OutlierGroup, QueueFindings, Report, RuleCounts, RulesFileRun, SfcCell, SnapshotRead,
    StageTime,
};
use crate::source_store::Sealer;
use crate::source_tables;
use crate::worker_report;

/// The largest report a worker may send.
const REPORT_MAX: usize = 256 * 1024 * 1024;

/// The audience workers ask their token for.
pub const AUDIENCE: &str = "quality-api";

/// Which machine a token belongs to (the installation's OIDC provider).
pub trait MachineTokens: Send + Sync {
    /// Who a live machine token for [`AUDIENCE`] was given to.
    fn client(&self, token: &str) -> Option<MachineCaller>;
}

/// A machine token's client, and the machine its certificate named when
/// the client stands for a role's machines (decision 0066, point 6; the
/// token's `machine` claim). A client with a secret has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineCaller {
    pub client: String,
    pub machine: Option<String>,
}

impl MachineCaller {
    /// What the caller's workers are named after, and how it is recorded:
    /// `<client>`, or `<client>/<machine>` as a role's client is shared by
    /// its machines. A job is the caller's if its worker starts with this
    /// and a `/`, so one machine of a role cannot read or report another's.
    /// Neither a client id nor a machine's name holds a `/`.
    pub fn worker_prefix(&self) -> String {
        match &self.machine {
            Some(machine) => format!("{}/{machine}", self.client),
            None => self.client.clone(),
        }
    }
}

/// What the workers' routes use besides the database.
#[derive(Clone)]
pub struct WorkerServices {
    pub tokens: Arc<dyn MachineTokens>,
    /// Without it, jobs come with no access to the lake.
    pub lake: Option<Arc<dyn LakeIssuer>>,
    /// Without it, jobs come with no rules.
    pub catalog: Option<Arc<dyn Catalog>>,
    /// Without it, evaluations leave no manifest.
    pub manifests: Option<Arc<dyn ManifestStore>>,
    /// The sources' key: without it, extractions come with no source.
    pub sources: Option<Arc<dyn Sealer>>,
    /// Where map jobs' proposals are written: without it, a map's report
    /// is refused (503).
    pub drafts: Option<Arc<dyn Drafts>>,
    /// The texts of the rules a map proposes, in the installation's
    /// language (`crates/texts`).
    pub proposal_alerts: Option<Alerts>,
    /// Where a table's files are: without it, extractions get no lake.
    pub tables: Option<Arc<dyn TableCatalog>>,
    /// Where each credential given to a job is recorded first: without it,
    /// no credential is given (decision 0030, point 3).
    pub issues: Option<Arc<dyn IssueLog>>,
}

impl std::fmt::Debug for WorkerServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerServices")
            .field("lake", &self.lake.is_some())
            .field("catalog", &self.catalog.is_some())
            .field("manifests", &self.manifests.is_some())
            .field("sources", &self.sources.is_some())
            .field("tables", &self.tables.is_some())
            .field("drafts", &self.drafts.is_some())
            .field("issues", &self.issues.is_some())
            .finish_non_exhaustive()
    }
}

/// What a worker needs to run a job it holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobDetails {
    pub kind: String,
    /// `None` only for a job of the whole warehouse (`maintain`).
    pub dataset: Option<String>,
    pub parameters: serde_json::Value,
    pub engine: String,
    pub attempts: i32,
    /// The job's own credentials for its tenant's warehouse.
    pub lake: Option<LakeAccess>,
    /// The dataset's mapping and rules on the catalog's `main` (`None` if
    /// the catalog has none for it).
    pub catalog: Option<CatalogSnapshot>,
    /// An extraction's source, as its mapping's `from` names it (0036).
    #[serde(default)]
    pub source: Option<SourceAccess>,
    /// An evaluation's key for its entities' hashes (decision 0057).
    #[serde(default)]
    pub entity_key: Option<EntityKey>,
}

#[derive(sqlx::FromRow)]
pub(crate) struct HeldJob {
    pub(crate) kind: String,
    pub(crate) dataset: Option<String>,
    pub(crate) parameters: serde_json::Value,
    engine: String,
    attempts: i32,
}

#[derive(Clone)]
pub(crate) struct WorkerApi {
    pub(crate) pool: PgPool,
    pub(crate) services: WorkerServices,
}

/// `GET /internal/jobs/{id}`, `GET …/lake` and `POST …/report` (decision
/// 0076: the job is the installation's, the main tenant's), and the same
/// under `/internal/jobs/{tenant}/{id}` for workers not yet updated, until
/// the tenants' contraction.
pub fn routes(pool: PgPool, services: WorkerServices) -> Router {
    Router::new()
        .route("/internal/jobs/{id}", get(details_of))
        .route("/internal/jobs/{id}/lake", get(lake_again_of))
        .route(
            "/internal/jobs/{id}/report",
            post(report_of).layer(DefaultBodyLimit::max(REPORT_MAX)),
        )
        .route("/internal/jobs/{tenant}/{id}", get(details))
        .route(
            "/internal/jobs/{tenant}/{id}/lake",
            get(job_lake::lake_again),
        )
        // An evaluation's report carries a row per entity (0057): a
        // client base of a million is some 100 MiB.
        .route(
            "/internal/jobs/{tenant}/{id}/report",
            post(worker_report::report).layer(DefaultBodyLimit::max(REPORT_MAX)),
        )
        // Each call runs in its job's span (decision 0026, point 1).
        .route_layer(axum::middleware::from_fn(job_span::enter))
        .with_state(WorkerApi { pool, services })
}

/// The tenant of every job of the installation (decision 0076).
fn main_tenant() -> Uuid {
    Uuid::from_u128(1)
}

/// `GET /internal/jobs/{id}`.
async fn details_of(
    api: State<WorkerApi>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<JobDetails>, StatusCode> {
    details(api, Path((main_tenant(), id)), headers).await
}

/// `GET /internal/jobs/{id}/lake`.
async fn lake_again_of(
    api: State<WorkerApi>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<LakeAccess>, StatusCode> {
    job_lake::lake_again(api, Path((main_tenant(), id)), headers).await
}

/// `POST /internal/jobs/{id}/report`.
async fn report_of(
    api: State<WorkerApi>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Json<serde_json::Value>,
) -> Result<StatusCode, StatusCode> {
    worker_report::report(api, Path((main_tenant(), id)), headers, body).await
}

impl WorkerApi {
    /// The caller, as [`MachineCaller::worker_prefix`]: what [`Self::held`]
    /// matches and what the credentials given are recorded to.
    pub(crate) fn caller(&self, headers: &HeaderMap) -> Result<String, StatusCode> {
        headers
            .get(AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .and_then(|t| self.services.tokens.client(t))
            .map(|c| c.worker_prefix())
            .ok_or(StatusCode::UNAUTHORIZED)
    }

    /// The job, if `client`'s worker holds it now: its worker is
    /// `<client>/…` (`<client>/<machine>/…` for a role's machine).
    pub(crate) async fn held(
        &self,
        tenant: Uuid,
        id: Uuid,
        client: &str,
    ) -> Result<HeldJob, StatusCode> {
        let job: HeldJob = sqlx::query_as(
            "SELECT kind, dataset, parameters, engine, attempts FROM quality.jobs
              WHERE tenant_id = $1 AND id = $2 AND state = 'running'
                AND lease_until > now() AND starts_with(worker, $3 || '/')",
        )
        .bind(tenant)
        .bind(id)
        .bind(client)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?
        .ok_or(StatusCode::NOT_FOUND)?;
        job_span::record(&job.kind, job.dataset.as_deref(), &job.parameters);
        Ok(job)
    }

    /// The dataset's mapping and rules on the catalog's `main`, if any.
    pub(crate) async fn snapshot(
        &self,
        dataset: &str,
    ) -> Result<Option<CatalogSnapshot>, StatusCode> {
        let Some(catalog) = &self.services.catalog else {
            return Ok(None);
        };
        catalog.snapshot(dataset).await.map_err(|e| {
            tracing::error!("catalog: {e}");
            StatusCode::SERVICE_UNAVAILABLE
        })
    }

    /// The source a `kind` job reads (an extraction's from its `mapping`),
    /// not yet recorded; `None` for a kind that reads none, or without the
    /// sources' key.
    pub(crate) async fn job_source(
        &self,
        kind: &str,
        mapping: Option<&str>,
        parameters: &serde_json::Value,
    ) -> Result<Option<SourceAccess>, StatusCode> {
        let Some(sealer) = self
            .services
            .sources
            .as_ref()
            .filter(|_| reads_source(kind))
        else {
            return Ok(None);
        };
        job_sources::source_for(&self.pool, sealer.as_ref(), kind, parameters, mapping)
            .await
            .map_err(|e| {
                tracing::error!("source: {e:?}");
                StatusCode::SERVICE_UNAVAILABLE
            })
    }
}

/// Whether a `kind` job reads a source.
fn reads_source(kind: &str) -> bool {
    matches!(
        kind,
        "extract" | "map" | source_tables::KIND | records_api::KIND
    )
}

pub(crate) fn internal(e: sqlx::Error) -> StatusCode {
    tracing::error!("worker route: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}

async fn details(
    State(api): State<WorkerApi>,
    Path((tenant, id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<JobDetails>, StatusCode> {
    let client = api.caller(&headers)?;
    async {
        let job = api.held(tenant, id, &client).await?;
        // A rule's test reads a cut as an evaluation does (decision 0043).
        // A console statement reads the whole lake as one too (0054).
        let evaluate = matches!(job.kind.as_str(), "evaluate" | "test_rule" | "query");
        let extract = job.kind == "extract";
        let catalog = match &job.dataset {
            Some(dataset) if evaluate || extract => api.snapshot(dataset).await?,
            _ => None,
        };
        let mapping = catalog.as_ref().map(|c| c.mapping.as_str());
        let source = api.job_source(&job.kind, mapping, &job.parameters).await?;
        source_given(&api, id, &job.kind, &client, &headers, source.as_ref()).await?;
        let lake = if evaluate {
            job_lake::read_access(&api, id, &job.kind, &client, &headers).await?
        } else {
            lake_of(&api, id, (&job, catalog.as_ref()), &client, &headers).await?
        };
        let entity_key = entity_key::for_job(&api, id, &job.kind, &client, &headers).await?;
        Ok(Json(JobDetails {
            kind: job.kind,
            dataset: job.dataset,
            parameters: job.parameters,
            engine: job.engine,
            attempts: job.attempts,
            lake,
            catalog,
            source,
            entity_key,
        }))
    }
    .await
}

/// The source a job reads, with its credentials, recorded first as it is
/// given (decision 0030, point 3).
async fn source_given(
    api: &WorkerApi,
    id: Uuid,
    kind: &str,
    client: &str,
    headers: &HeaderMap,
    source: Option<&SourceAccess>,
) -> Result<(), StatusCode> {
    let Some(s) = source else {
        return Ok(());
    };
    let log = api.services.issues.as_deref();
    let given = format!("source/{}", s.name);
    job_sources::record(log, id, kind, client, headers, &given).await
}

/// The lake of a job that is no evaluation: an extraction's, its bronze
/// table (decision 0037); a maintenance's, its dataset's tables (0039); a
/// listing of the lake's tables, the lake's.
async fn lake_of(
    api: &WorkerApi,
    id: Uuid,
    (job, catalog): (&HeldJob, Option<&CatalogSnapshot>),
    client: &str,
    headers: &HeaderMap,
) -> Result<Option<LakeAccess>, StatusCode> {
    match (catalog, job.kind.as_str(), &job.dataset) {
        (Some(snapshot), "extract", _) => {
            job_lake::extract_access(api, id, client, headers, &snapshot.mapping).await
        }
        (_, "maintain", Some(dataset)) => {
            job_maintain::access(api, id, client, headers, dataset).await
        }
        (_, lake_tables::KIND, _) => lake_tables::access(api, id, client, headers).await,
        _ => Ok(None),
    }
}
