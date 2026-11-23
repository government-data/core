//! A tenant's schedules for people (decision 0035), in the module's
//! `user_router`: list, create, edit and delete. What a caller may do is
//! asked of its [`Caller`]: `quality:ViewPipelines` and
//! `quality:ManageSchedules` on `pipeline/<dataset>`, or on every pipeline
//! (`*`) for a job of the whole warehouse. A schedule's dataset must be in
//! the catalog; every change is audited.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use contracts::{AuditEntry, Caller, Change};
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

use crate::catalog::Catalog;
use crate::schedule::{Rerun, ScheduleError, ScheduleTrigger, Target};
use crate::schedule_store::{self, Deleted, ScheduleSpec, SpecError, StoredSchedule};

mod body;

use body::spec_of;
pub use body::ScheduleBody;

pub const VIEW: &str = "quality:ViewPipelines";
pub const MANAGE: &str = "quality:ManageSchedules";

#[derive(Clone)]
struct ScheduleApi {
    pool: PgPool,
    catalog: Option<Arc<dyn Catalog>>,
}

/// `GET`/`POST /quality/schedules`, `PUT`/`DELETE /quality/schedules/{id}`.
pub fn routes(pool: PgPool, catalog: Option<Arc<dyn Catalog>>) -> Router {
    Router::new()
        .route("/quality/schedules", get(list).post(create))
        .route("/quality/schedules/{id}", put(update).delete(remove))
        .with_state(ScheduleApi { pool, catalog })
}

/// The routes' OpenAPI paths and schemas.
#[derive(Debug, OpenApi)]
#[openapi(
    paths(list, create, update, remove),
    components(schemas(ScheduleBody, ScheduleOut, ScheduleRefusal)),
    tags((name = "schedules", description = "When a dataset's cut, or a job, fires")),
)]
pub struct ScheduleDoc;

/// A schedule, with what the scheduler keeps of it.
#[derive(Debug, Serialize, ToSchema)]
pub struct ScheduleOut {
    #[schema(value_type = String)]
    pub id: Uuid,
    /// Empty when it fires by its trigger.
    pub cron: String,
    #[schema(value_type = Option<Object>)]
    pub trigger: Option<ScheduleTrigger>,
    pub timezone: String,
    pub enabled: bool,
    pub fires: String,
    pub dataset: Option<String>,
    pub rerun: Option<String>,
    pub kind: Option<String>,
    #[schema(value_type = Option<Object>)]
    pub parameters: Option<serde_json::Value>,
    #[schema(value_type = Option<String>, format = DateTime)]
    pub next_fire_at: Option<DateTime<Utc>>,
    #[schema(value_type = Option<String>, format = DateTime)]
    pub last_fired_at: Option<DateTime<Utc>>,
    /// `queued`, `blocked`, `replaced` or `skipped`.
    pub last_outcome: Option<String>,
}

/// Why a schedule is refused: a code the UI's texts translate.
#[derive(Debug, Serialize, ToSchema)]
pub struct ScheduleRefusal {
    /// `fires`, `rerun`, `fields`, `cron`, `trigger`, `timezone`,
    /// `never_fires`, `kind`, `dataset` or `unknown_dataset`.
    pub error: String,
}

impl From<StoredSchedule> for ScheduleOut {
    fn from(s: StoredSchedule) -> Self {
        let (fires, dataset, rerun, kind, parameters) = match s.spec.target {
            Target::Cut { dataset, rerun } => {
                let rerun = if rerun == Rerun::Replace {
                    "replace"
                } else {
                    "block"
                };
                ("cut", Some(dataset), Some(rerun.to_string()), None, None)
            }
            Target::Job {
                kind,
                dataset,
                parameters,
            } => ("job", dataset, None, Some(kind), Some(parameters)),
        };
        Self {
            id: s.id,
            cron: s.spec.cron,
            trigger: s.spec.trigger,
            timezone: s.spec.timezone,
            enabled: s.spec.enabled,
            fires: fires.into(),
            dataset,
            rerun,
            kind,
            parameters,
            next_fire_at: s.next_fire_at,
            last_fired_at: s.last_fired_at,
            last_outcome: s.last_outcome,
        }
    }
}

/// A refusal: its status and a code the UI's texts translate; an internal
/// one logs what happened and says nothing more.
#[derive(Debug, Clone, Copy)]
struct Refused(StatusCode, &'static str);

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

fn internal(what: impl std::fmt::Display) -> Refused {
    tracing::error!("schedules: {what}");
    Refused(StatusCode::INTERNAL_SERVER_ERROR, "internal")
}

const FORBIDDEN: Refused = Refused(StatusCode::FORBIDDEN, "forbidden");
const NOT_FOUND: Refused = Refused(StatusCode::NOT_FOUND, "not_found");
const NO_CATALOG: Refused = Refused(StatusCode::SERVICE_UNAVAILABLE, "catalog_unavailable");

/// The platform's time zone, for a schedule that names none (decision 0079).
async fn platform_timezone(pool: &PgPool) -> Result<String, Refused> {
    schedule_store::platform_timezone(pool)
        .await
        .map_err(internal)
}

/// The pipeline a schedule acts on, for IAM.
fn pipeline(spec: &ScheduleSpec) -> String {
    spec.dataset()
        .map_or("*".into(), |d| format!("pipeline/{d}"))
}

impl ScheduleApi {
    /// The spec's first fire, if it holds together and its dataset is in
    /// the catalog.
    async fn checked(&self, spec: &ScheduleSpec) -> Result<DateTime<Utc>, Refused> {
        let next = spec.check(Utc::now()).map_err(|e| {
            let code = match e {
                SpecError::Schedule(ScheduleError::Fields) => "fields",
                SpecError::Schedule(ScheduleError::Cron(_)) => "cron",
                SpecError::Schedule(ScheduleError::Timezone(_)) => "timezone",
                SpecError::Schedule(ScheduleError::NeverFires) => "never_fires",
                SpecError::Schedule(ScheduleError::Trigger(_)) => "trigger",
                SpecError::Kind => "kind",
                SpecError::Dataset => "dataset",
            };
            Refused(StatusCode::UNPROCESSABLE_ENTITY, code)
        })?;
        let Some(dataset) = spec.dataset() else {
            return Ok(next);
        };
        let catalog = self.catalog.as_ref().ok_or(NO_CATALOG)?;
        match catalog.snapshot(dataset).await {
            Ok(Some(_)) => Ok(next),
            Ok(None) => Err(Refused(StatusCode::UNPROCESSABLE_ENTITY, "unknown_dataset")),
            Err(e) => {
                tracing::error!("schedules: catalog: {e}");
                Err(NO_CATALOG)
            }
        }
    }
}

async fn audited(
    caller: &Caller,
    resource: String,
    id: Uuid,
    change: Change,
) -> Result<(), Refused> {
    let entry = AuditEntry {
        action: MANAGE,
        resource: format!("{resource}/schedule/{id}"),
        change,
    };
    caller
        .audit(entry)
        .await
        .map_err(|e| internal(format!("audit: {e}")))
}

/// The schedules the caller may see.
#[utoipa::path(
    get,
    path = "/quality/schedules",
    operation_id = "quality_list_schedules",
    responses(
        (status = 200, description = "The tenant's schedules the caller may see", body = [ScheduleOut]),
        (status = 401, description = "No valid session"),
        (status = 403, description = "May see no pipeline"),
    ),
    tag = "schedules",
)]
async fn list(
    State(api): State<ScheduleApi>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Vec<ScheduleOut>>, Refused> {
    if !caller.can_some(VIEW) {
        return Err(FORBIDDEN);
    }
    let all = schedule_store::list(&api.pool).await.map_err(internal)?;
    Ok(Json(
        all.into_iter()
            .filter(|s| caller.can(VIEW, &pipeline(&s.spec)))
            .map(ScheduleOut::from)
            .collect(),
    ))
}

/// Creates a schedule; its first fire comes back.
#[utoipa::path(
    post,
    path = "/quality/schedules",
    operation_id = "quality_create_schedule",
    request_body = ScheduleBody,
    responses(
        (status = 201, description = "Created", body = ScheduleOut),
        (status = 401, description = "No valid session"),
        (status = 403, description = "May not schedule this pipeline"),
        (status = 422, description = "It does not hold together", body = ScheduleRefusal),
        (status = 503, description = "No catalog to know the dataset"),
    ),
    tag = "schedules",
)]
async fn create(
    State(api): State<ScheduleApi>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<ScheduleBody>,
) -> Result<Response, Refused> {
    let spec = spec_of(body, platform_timezone(&api.pool).await?)?;
    if !caller.can(MANAGE, &pipeline(&spec)) {
        return Err(FORBIDDEN);
    }
    let next = api.checked(&spec).await?;
    let id = schedule_store::create(&api.pool, &spec, next)
        .await
        .map_err(internal)?;
    audited(&caller, pipeline(&spec), id, Change::Created).await?;
    let stored = schedule_store::get(&api.pool, id)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal("a schedule just created is not there"))?;
    Ok((StatusCode::CREATED, Json(ScheduleOut::from(stored))).into_response())
}

/// Replaces a schedule; its next fire is computed again.
#[utoipa::path(
    put,
    path = "/quality/schedules/{id}",
    operation_id = "quality_update_schedule",
    params(("id" = String, Path, description = "The schedule's id")),
    request_body = ScheduleBody,
    responses(
        (status = 200, description = "Replaced", body = ScheduleOut),
        (status = 401, description = "No valid session"),
        (status = 403, description = "May not schedule this pipeline, or the one it was"),
        (status = 404, description = "No such schedule"),
        (status = 422, description = "It does not hold together", body = ScheduleRefusal),
        (status = 503, description = "No catalog to know the dataset"),
    ),
    tag = "schedules",
)]
async fn update(
    State(api): State<ScheduleApi>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<Uuid>,
    Json(body): Json<ScheduleBody>,
) -> Result<Json<ScheduleOut>, Refused> {
    let spec = spec_of(body, platform_timezone(&api.pool).await?)?;
    let before = schedule_store::get(&api.pool, id)
        .await
        .map_err(internal)?
        .ok_or(NOT_FOUND)?;
    if !caller.can(MANAGE, &pipeline(&before.spec)) || !caller.can(MANAGE, &pipeline(&spec)) {
        return Err(FORBIDDEN);
    }
    let next = api.checked(&spec).await?;
    if !schedule_store::update(&api.pool, id, &spec, next)
        .await
        .map_err(internal)?
    {
        return Err(NOT_FOUND);
    }
    audited(&caller, pipeline(&spec), id, Change::Changed).await?;
    let stored = schedule_store::get(&api.pool, id)
        .await
        .map_err(internal)?
        .ok_or(NOT_FOUND)?;
    Ok(Json(ScheduleOut::from(stored)))
}

/// Deletes a schedule that has fired no cut; one that has is disabled.
#[utoipa::path(
    delete,
    path = "/quality/schedules/{id}",
    operation_id = "quality_delete_schedule",
    params(("id" = String, Path, description = "The schedule's id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "No valid session"),
        (status = 403, description = "May not schedule this pipeline"),
        (status = 404, description = "No such schedule"),
        (status = 409, description = "It has cuts: disable it instead", body = ScheduleRefusal),
    ),
    tag = "schedules",
)]
async fn remove(
    State(api): State<ScheduleApi>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, Refused> {
    let before = schedule_store::get(&api.pool, id)
        .await
        .map_err(internal)?
        .ok_or(NOT_FOUND)?;
    let resource = pipeline(&before.spec);
    if !caller.can(MANAGE, &resource) {
        return Err(FORBIDDEN);
    }
    match schedule_store::delete(&api.pool, id)
        .await
        .map_err(internal)?
    {
        Deleted::Deleted => {}
        Deleted::NotFound => return Err(NOT_FOUND),
        Deleted::HasCuts => return Err(Refused(StatusCode::CONFLICT, "has_cuts")),
    }
    audited(&caller, resource, id, Change::Deleted).await?;
    Ok(StatusCode::NO_CONTENT)
}
