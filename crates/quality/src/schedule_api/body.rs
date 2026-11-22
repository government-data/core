//! A schedule as a person writes it, and the spec it asks for.

use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::json;
use utoipa::ToSchema;

use super::Refused;
use crate::schedule::{Rerun, ScheduleTrigger, Target};
use crate::schedule_store::ScheduleSpec;

/// A schedule as a person writes it.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ScheduleBody {
    /// Five fields: minute hour day month weekday (`0 6 * * 1-5`); left
    /// out with a trigger.
    #[serde(default)]
    pub cron: String,
    /// In the style of the Windows Task Scheduler: a start, how often
    /// (`once`, `daily`, `weekly`, `monthly`), and repetition, a random
    /// delay and expiry. When given, the schedule fires by it, not by `cron`.
    #[schema(value_type = Option<Object>)]
    pub trigger: Option<ScheduleTrigger>,
    /// IANA; the platform's time zone when left out (UTC if it has none).
    pub timezone: Option<String>,
    /// `true` when left out; a schedule with cuts is disabled, not deleted.
    pub enabled: Option<bool>,
    /// `cut` (extraction, then evaluation) or `job` (one that needs no cut).
    pub fires: String,
    /// A cut's and a `map`'s; none for a `maintain` of the whole warehouse.
    pub dataset: Option<String>,
    /// A cut's: `block` (the default) or `replace`.
    pub rerun: Option<String>,
    /// A job's: `map` or `maintain`.
    pub kind: Option<String>,
    /// A job's parameters.
    #[schema(value_type = Option<Object>)]
    pub parameters: Option<serde_json::Value>,
}

/// The spec a body asks for; `timezone` when it names none.
pub(super) fn spec_of(body: ScheduleBody, timezone: String) -> Result<ScheduleSpec, Refused> {
    let target = match body.fires.as_str() {
        "cut" => Target::Cut {
            dataset: body.dataset.unwrap_or_default(),
            rerun: match body.rerun.as_deref() {
                None | Some("block") => Rerun::Block,
                Some("replace") => Rerun::Replace,
                Some(_) => return Err(Refused(StatusCode::UNPROCESSABLE_ENTITY, "rerun")),
            },
        },
        "job" => Target::Job {
            kind: body.kind.unwrap_or_default(),
            dataset: body.dataset,
            parameters: body.parameters.unwrap_or_else(|| json!({})),
        },
        _ => return Err(Refused(StatusCode::UNPROCESSABLE_ENTITY, "fires")),
    };
    Ok(ScheduleSpec {
        cron: body.cron,
        trigger: body.trigger,
        timezone: body.timezone.unwrap_or(timezone),
        enabled: body.enabled.unwrap_or(true),
        target,
    })
}
