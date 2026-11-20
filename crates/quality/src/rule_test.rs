//! A rule tested as it is written (decisions 0019 point 10, 0043 point 3):
//! a rules file as typed, run on the dataset's latest cut by a `test_rule`
//! job on a worker, which brings each rule's counts and a read of a few
//! records of each outcome. Asking needs `quality:ViewRules` on
//! `catalog/<dataset>`; a text that does not lint is refused before it is
//! queued. The result is read only by whoever asked, masked, and revealed
//! with a reason behind `quality:ViewPersonalData` on `pipeline/<dataset>`
//! ([`crate::rule_test_view`]).

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use contracts::{Caller, ViewPurpose};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

use crate::draft_api::{DraftCatalog, Refused, VIEW};
use crate::edit_api;
use crate::reveal::RevealIn;
use crate::rule_test_view as view;
use crate::source::valid_name;

pub const PERSONAL: &str = "quality:ViewPersonalData";
/// Records of each outcome a test reads (decision 0043, Open).
pub const EACH: usize = 10;

#[derive(Clone)]
pub(crate) struct TestApi {
    pub(crate) pool: PgPool,
    catalog: Arc<dyn DraftCatalog>,
}

/// `POST /quality/rules/test`, `GET /quality/rules/tests/{job}` and
/// `POST …/{job}/reveal`.
pub fn routes(pool: PgPool, catalog: Arc<dyn DraftCatalog>) -> Router {
    Router::new()
        .route("/quality/rules/test", post(start))
        .route("/quality/rules/tests/{job}", get(view::read))
        .route("/quality/rules/tests/{job}/reveal", post(view::reveal))
        .with_state(TestApi { pool, catalog })
}

/// The routes' OpenAPI paths and schemas.
#[derive(Debug, OpenApi)]
#[openapi(
    paths(start, view::read, view::reveal),
    components(schemas(
        RevealIn,
        ViewPurpose,
        TestBody,
        TestStarted,
        TestOut,
        TestReport,
        RuleTested,
        TestedRecord,
        FailGroup,
        FieldValue
    ))
)]
pub struct TestDoc;

/// A rules file as typed, to test on `dataset`'s latest cut; `rule` only
/// that rule.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TestBody {
    pub dataset: String,
    pub yaml: String,
    pub rule: Option<String>,
    /// Fields of the dataset to show in the records read, besides the key
    /// and those a rule reads; any of the mapping's, all of them if asked.
    #[serde(default)]
    pub columns: Vec<String>,
}

/// The test queued: its job and the cut it reads.
#[derive(Debug, Serialize, ToSchema)]
pub struct TestStarted {
    pub job: String,
    pub cut: String,
}

/// A field's value in a record read; `null` where empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FieldValue {
    pub field: String,
    pub value: Option<String>,
}

/// A record a rule was run on: `fails`, `passes` or `skips` (does not
/// apply), with its key's values first, then those of the fields the rule
/// reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TestedRecord {
    pub outcome: String,
    pub values: Vec<FieldValue>,
}

/// One rule's counts on the cut and its read of records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RuleTested {
    pub rule: String,
    pub applies: u64,
    pub passes: u64,
    pub fails: u64,
    pub skips: u64,
    pub records: Vec<TestedRecord>,
    /// Its failures grouped by the shape of what it read, largest first.
    #[serde(default)]
    pub groups: Vec<FailGroup>,
}

/// Failures of one shape (each digit `9`, each letter `A`): how many, and
/// a few of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FailGroup {
    pub shape: String,
    pub count: u64,
    pub records: Vec<TestedRecord>,
}

/// What a `test_rule` job found, as its worker reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TestReport {
    pub cut: String,
    /// The records of the cut the test read.
    pub records: u64,
    pub rules: Vec<RuleTested>,
}

/// A test as its asker sees it: `queued`, `running`, `done`, `failed` or
/// `expired`.
#[derive(Debug, Serialize, ToSchema)]
pub struct TestOut {
    /// `queued`, `running`, `done`, `failed`, or `expired`: done, its report
    /// no longer kept (a day), so none is answered; test again.
    pub state: String,
    pub error: Option<String>,
    /// Whether the values are masked: always on a read, never on a reveal.
    pub masked: bool,
    /// Whether a reveal would show values: the report has records and the
    /// caller holds the permission that shows them whole.
    pub revealable: bool,
    pub report: Option<TestReport>,
}

/// `••••` and the last four characters of a value, or only `••••`.
pub fn mask(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 4 {
        "••••".to_string()
    } else {
        format!(
            "••••{}",
            chars[chars.len() - 4..].iter().collect::<String>()
        )
    }
}

fn refuse(code: StatusCode, error: &str) -> Response {
    (code, Json(json!({ "error": error }))).into_response()
}

fn internal(e: sqlx::Error) -> Response {
    tracing::error!("rule test: {e}");
    refuse(StatusCode::SERVICE_UNAVAILABLE, "unavailable")
}

/// The dataset's latest cut: extracted, with its bronze snapshot, or else
/// evaluated (a cut loaded by hand), read as it is now.
async fn latest_cut(
    pool: &PgPool,
    dataset: &str,
) -> Result<Option<(String, Option<String>)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT cut::text, snapshot FROM (
             SELECT cut, bronze_snapshot AS snapshot FROM quality.cuts
              WHERE dataset = $1 AND bronze_snapshot IS NOT NULL
                AND state IN ('evaluating', 'done')
             UNION ALL
             SELECT cut, NULL FROM quality.run_summary WHERE dataset = $1
         ) known ORDER BY cut DESC, snapshot IS NULL LIMIT 1",
    )
    .bind(dataset)
    .fetch_optional(pool)
    .await
}

/// Queues a test of a rules file as typed on the dataset's latest cut.
#[utoipa::path(post, path = "/quality/rules/test", operation_id = "quality_test_rules",
    tag = "rules", request_body = TestBody,
    responses((status = 202, body = TestStarted), (status = 401), (status = 403),
        (status = 404, description = "The dataset is not on `main`"),
        (status = 409, description = "`no_cut`: the dataset has no cut extracted or evaluated yet"),
        (status = 422, description = "`lint`: the errors that keep it from running, as `findings`; `columns`: a field the mapping does not have"),
        (status = 503)))]
async fn start(
    State(api): State<TestApi>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<TestBody>,
) -> Response {
    let dataset = body.dataset;
    if !valid_name(&dataset) || !caller.can(VIEW, &format!("catalog/{dataset}")) {
        return refuse(StatusCode::FORBIDDEN, "forbidden");
    }
    // A test saves no file: any name will do.
    match edit_api::check(api.catalog.as_ref(), &dataset, ("", None), &body.yaml).await {
        // Only the mapping's fields are shown.
        Ok(checked) if body.columns.iter().any(|c| !checked.fields.contains(c)) => {
            return refuse(StatusCode::UNPROCESSABLE_ENTITY, "columns");
        }
        Ok(checked) if checked.errors.is_empty() => {}
        Ok(checked) => {
            let out = json!({ "error": "lint", "findings": checked.errors });
            return (StatusCode::UNPROCESSABLE_ENTITY, Json(out)).into_response();
        }
        Err(Refused(code, error)) => return refuse(code, error),
    }
    let (cut, snapshot) = match latest_cut(&api.pool, &dataset).await {
        Ok(Some(found)) => found,
        Ok(None) => return refuse(StatusCode::CONFLICT, "no_cut"),
        Err(e) => return internal(e),
    };
    let parameters = json!({
        "cut": cut,
        "snapshot": snapshot,
        "yaml": body.yaml,
        "rule": body.rule,
        "each": EACH,
        "columns": body.columns,
        "by": caller.account,
    });
    // A test is waited for: before the scheduled work, and run once.
    let job: Result<Uuid, _> = sqlx::query_scalar(
        "INSERT INTO quality.jobs (kind, dataset, parameters, priority, max_attempts)
         VALUES ('test_rule', $1, $2, 10, 1) RETURNING id",
    )
    .bind(&dataset)
    .bind(&parameters)
    .fetch_one(&api.pool)
    .await;
    match job {
        Ok(job) => {
            let out = TestStarted {
                job: job.to_string(),
                cut,
            };
            (StatusCode::ACCEPTED, Json(out)).into_response()
        }
        Err(e) => internal(e),
    }
}

/// A `test_rule` job's report, kept for its asker; older tests purged.
pub(crate) async fn report(
    pool: &PgPool,
    id: Uuid,
    asked_cut: Option<&str>,
    body: serde_json::Value,
) -> Result<(), StatusCode> {
    let report: TestReport =
        serde_json::from_value(body).map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    let outcomes_known = report.rules.iter().all(|r| {
        r.records
            .iter()
            .all(|t| matches!(t.outcome.as_str(), "fails" | "passes" | "skips"))
            && r.groups
                .iter()
                .flat_map(|g| &g.records)
                .all(|t| t.outcome == "fails")
    });
    if !outcomes_known || asked_cut.is_some_and(|c| c != report.cut) {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let value = serde_json::to_value(&report).map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    let stored = async {
        sqlx::query("DELETE FROM quality.rule_tests WHERE created_at < now() - interval '1 day'")
            .execute(pool)
            .await?;
        sqlx::query(
            "INSERT INTO quality.rule_tests (job, report) VALUES ($1, $2)
             ON CONFLICT (tenant_id, job) DO UPDATE SET report = EXCLUDED.report",
        )
        .bind(id)
        .bind(value)
        .execute(pool)
        .await
    };
    stored.await.map(|_| ()).map_err(|e| {
        tracing::error!("rule test report: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}
