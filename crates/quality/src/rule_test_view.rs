//! A rule test's result as its asker reads it (decisions 0043, 0069): the
//! read always answers its records masked, and a reveal with a reason
//! answers them whole behind `quality:ViewPersonalData` on
//! `pipeline/<dataset>`, after one audit row with the reason and the shape.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use contracts::Caller;
use uuid::Uuid;

use crate::reveal::{self, Refusal, RevealIn, Shown};
use crate::rule_test::{mask, TestApi, TestOut, TestReport, TestedRecord, PERSONAL};

/// A test's job as its asker reads it: its dataset, state, error and
/// report (none until its worker reports).
#[derive(sqlx::FromRow)]
struct TestRow {
    dataset: String,
    state: String,
    error: Option<String>,
    report: Option<serde_json::Value>,
}

/// The caller's test `job`, its report if kept and readable, and whether
/// the caller sees its values; `Err` with the answer when not found. A
/// stored report that no longer reads is logged by its job alone and
/// answered as purged.
async fn load(
    api: &TestApi,
    caller: &Caller,
    job: Uuid,
) -> Result<(TestRow, Option<TestReport>, bool), Refusal> {
    let found: Result<Option<TestRow>, _> = sqlx::query_as(
        "SELECT j.dataset, j.state, j.error, t.report FROM quality.jobs j
               LEFT JOIN quality.rule_tests t ON t.tenant_id = j.tenant_id AND t.job = j.id
              WHERE j.id = $1 AND j.kind = 'test_rule' AND j.parameters->>'by' = $2",
    )
    .bind(job)
    .bind(&caller.account)
    .fetch_optional(&api.pool)
    .await;
    let mut row = match found {
        Ok(Some(row)) => row,
        Ok(None) => return Err(Refusal(StatusCode::NOT_FOUND, "not_found")),
        Err(e) => {
            tracing::error!("rule test: {e}");
            return Err(Refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable"));
        }
    };
    let report = row.report.take().and_then(|r| {
        serde_json::from_value(r)
            .inspect_err(|_| tracing::warn!(%job, "rule test: a stored report does not read"))
            .ok()
    });
    let sees = caller.can(PERSONAL, &format!("pipeline/{}", row.dataset));
    Ok((row, report, sees))
}

/// Every record the report lists, under its rules and its groups: a record
/// listed twice is shown twice.
fn listed(report: &TestReport) -> impl Iterator<Item = &TestedRecord> {
    report.rules.iter().flat_map(|t| {
        let grouped = t.groups.iter().flat_map(|g| g.records.iter());
        t.records.iter().chain(grouped)
    })
}

/// The report with every value masked.
fn masked(mut report: TestReport) -> TestReport {
    for value in report
        .rules
        .iter_mut()
        .flat_map(|t| {
            let grouped = t.groups.iter_mut().flat_map(|g| g.records.iter_mut());
            t.records.iter_mut().chain(grouped)
        })
        .flat_map(|r| r.values.iter_mut())
    {
        value.value = value.value.as_deref().map(mask);
    }
    report
}

/// The answer of a read or a reveal; `revealable` when it has records and
/// the caller sees them, once the job is finished. A done job with no
/// report is `expired`: its worker reports before finishing, so its report
/// was purged after a day (or no longer reads), never still on its way.
fn answer(row: TestRow, report: Option<TestReport>, sees: bool, masked: bool) -> Response {
    let records = report.as_ref().is_some_and(|r| listed(r).next().is_some());
    let revealable = sees && records && reveal::finished(&row.state);
    let state = if row.state == "done" && report.is_none() {
        "expired".into()
    } else {
        row.state
    };
    Json(TestOut {
        state,
        error: row.error,
        masked,
        revealable,
        report,
    })
    .into_response()
}

/// A test's state and, when done, what it found, always masked;
/// `revealable` when it has records and the caller holds
/// `quality:ViewPersonalData` on the dataset; `expired`, with no report,
/// once its report is purged (kept a day). Writes no audit row.
#[utoipa::path(get, path = "/quality/rules/tests/{job}", operation_id = "quality_read_rule_test",
    tag = "rules", params(("job" = String, Path)),
    responses((status = 200, body = TestOut), (status = 401),
        (status = 404, description = "No such test, or another person's"), (status = 503)))]
pub(crate) async fn read(
    State(api): State<TestApi>,
    Extension(caller): Extension<Caller>,
    Path(job): Path<Uuid>,
) -> Response {
    match load(&api, &caller, job).await {
        Ok((row, report, sees)) => answer(row, report.map(masked), sees, true),
        Err(refused) => refused.into_response(),
    }
}

/// A test's records with their values whole, for a reason: audited first,
/// one row per reveal with the purpose, the reference, the job, the records
/// listed (a record under two rules counted twice) and the fields shown.
#[utoipa::path(post, path = "/quality/rules/tests/{job}/reveal", operation_id = "quality_reveal_rule_test",
    tag = "rules", params(("job" = String, Path)), request_body = RevealIn,
    responses((status = 200, body = TestOut), (status = 401),
        (status = 403, description = "`forbidden`: no `quality:ViewPersonalData` on the dataset"),
        (status = 404, description = "No such test, another person's, or purged"),
        (status = 409, description = "`not_done`: queued, running, or failed with no report"),
        (status = 422, description = "`purpose` (not a key) or `reference` (not of its form, or none with `other`)"),
        (status = 503, description = "`audit_unavailable`: the view could not be audited, nothing shown")))]
pub(crate) async fn reveal(
    State(api): State<TestApi>,
    Extension(caller): Extension<Caller>,
    Path(job): Path<Uuid>,
    Json(body): Json<RevealIn>,
) -> Response {
    let (row, report, sees) = match load(&api, &caller, job).await {
        Ok(found) => found,
        Err(refused) => return refused.into_response(),
    };
    let shown = report.as_ref().map(|report| Shown {
        resource: format!("dataset/{}/test/{job}", row.dataset),
        rows: listed(report).count(),
        fields: listed(report)
            .flat_map(|r| r.values.iter().map(|v| v.field.clone()))
            .collect(),
    });
    if let Err(refused) = reveal::admit(&caller, job, &row.state, sees, &body, shown).await {
        return refused.into_response();
    }
    answer(row, report, sees, false)
}
