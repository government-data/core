//! The reveal of values with a reason (decision 0069): every view answers
//! masked, and a `POST …/reveal` with a purpose and an optional reference
//! answers them whole, once its job is done, after one audit row with the
//! reason, the job and what was shown. Shared by a source's page, a rule
//! test and a console result.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use contracts::{AuditEntry, Caller, Change, ViewDetail, ViewReason};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::rule_test::PERSONAL;

/// Why values are revealed: a purpose's key and, for a ticket or case, a
/// reference (required with `other`).
#[derive(Debug, Deserialize, ToSchema)]
pub struct RevealIn {
    /// A key of the fixed list; another is refused with 422 `purpose`.
    #[serde(default)]
    #[schema(value_type = contracts::ViewPurpose)]
    pub purpose: String,
    /// At most 32 ASCII characters (`INC-2041`): letters, digits and
    /// `-_/#.`, the first a letter or digit, at least one letter.
    pub reference: Option<String>,
}

/// What a reveal would show: the audit's resource, the rows and the fields,
/// in order (repeats dropped here).
#[derive(Debug)]
pub(crate) struct Shown {
    pub resource: String,
    pub rows: usize,
    pub fields: Vec<String>,
}

/// A refusal: its status and its code, answered as `{"error": code}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Refusal(pub StatusCode, pub &'static str);

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        crate::refusal::refuse(self.0, self.1)
    }
}

/// Whether a job in `state` may be revealed: done, or failed with something
/// kept.
pub(crate) fn finished(state: &str) -> bool {
    matches!(state, "done" | "failed")
}

/// Checks a reveal of the caller's `job` and audits it, before the values
/// go. `shown` is `None` when nothing is kept. Refused, in this order and
/// with no row: 404 when a done job's values were purged, 403 `forbidden`
/// without the permission (`sees`), 409 `not_done` while queued or running
/// or failed with nothing kept, 422 `purpose` or `reference`; 503
/// `audit_unavailable` when the row cannot be written.
pub(crate) async fn admit(
    caller: &Caller,
    job: Uuid,
    state: &str,
    sees: bool,
    body: &RevealIn,
    shown: Option<Shown>,
) -> Result<(), Refusal> {
    if shown.is_none() && state == "done" {
        return Err(Refusal(StatusCode::NOT_FOUND, "not_found"));
    }
    if !sees {
        return Err(Refusal(StatusCode::FORBIDDEN, "forbidden"));
    }
    let Some(shown) = shown.filter(|_| finished(state)) else {
        return Err(Refusal(StatusCode::CONFLICT, "not_done"));
    };
    let reason = ViewReason::parse(&body.purpose, body.reference.as_deref())
        .map_err(|e| Refusal(StatusCode::UNPROCESSABLE_ENTITY, e.code()))?;
    let mut fields: Vec<String> = Vec::with_capacity(shown.fields.len());
    for field in shown.fields {
        if !fields.contains(&field) {
            fields.push(field);
        }
    }
    let detail = ViewDetail {
        reason,
        job: job.to_string(),
        rows_shown: u32::try_from(shown.rows).unwrap_or(u32::MAX),
        fields_shown: fields,
    };
    let entry = AuditEntry {
        action: PERSONAL,
        resource: shown.resource,
        change: Change::Viewed,
    };
    caller.audit_view(entry, detail).await.map_err(|e| {
        tracing::error!("reveal: audit: {e}");
        Refusal(StatusCode::SERVICE_UNAVAILABLE, "audit_unavailable")
    })
}
