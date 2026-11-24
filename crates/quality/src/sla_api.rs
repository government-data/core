//! A dataset's remediation SLAs (decision 0059), in the module's
//! `user_router`: read behind `quality:ViewResults` and replaced behind
//! `quality:EditPipelines` on `pipeline/<dataset>`, audited, both asked of
//! the request's [`Caller`]. An SLA is days per dimension (`group` none)
//! and per group of it, which wins; 30 days when none is set. The chart
//! `remediation_queues` applies them when it is read, so a change needs no
//! evaluation.
//!
//! `PUT` replaces the dataset's whole set: the form sends every SLA it
//! shows, and one left out goes back to its default. Each SLA's dimension
//! is a field holding the `dimension` role in the dataset's mapping on
//! `main`; its days are from 1 to [`MAX_DAYS`].

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use contracts::{AuditEntry, Caller, Change};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use utoipa::{OpenApi, ToSchema};

use crate::catalog::Catalog;
use crate::mapping::{FieldRole, Mapping};

pub const VIEW: &str = "quality:ViewResults";
pub const EDIT: &str = "quality:EditPipelines";
/// A queue's SLA when none is set, as the chart reads it.
pub const DEFAULT_DAYS: i64 = 30;
/// The longest SLA: a year. A queue given longer is no queue to work.
pub const MAX_DAYS: i64 = 365;
/// The most SLAs of a dataset: its dimensions' defaults and the groups
/// overridden, well above the thousand groups a dimension keeps.
pub const MAX_SLAS: usize = 2000;
/// A group's longest value, in characters.
const MAX_GROUP: usize = 256;

/// `GET`/`PUT /quality/datasets/{dataset}/queues/slas`.
pub fn routes(pool: PgPool, catalog: Option<Arc<dyn Catalog>>) -> Router {
    Router::new()
        .route(
            "/quality/datasets/{dataset}/queues/slas",
            get(read).put(replace),
        )
        .with_state(SlaApi { pool, catalog })
}

#[derive(Clone)]
struct SlaApi {
    pool: PgPool,
    catalog: Option<Arc<dyn Catalog>>,
}

/// The routes' OpenAPI paths and schemas.
#[derive(Debug, OpenApi)]
#[openapi(
    paths(read, replace),
    components(schemas(Sla, SlasBody, SlasOut, SlaRefusal)),
    tags((name = "queues", description = "A dataset's remediation queues and their SLAs")),
)]
pub struct SlaDoc;

/// One queue's SLA, or a dimension's default.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, ToSchema, sqlx::FromRow)]
pub struct Sla {
    /// A field holding the `dimension` role.
    pub dimension: String,
    /// One of its values (`""` is the empty value's group); none for the
    /// dimension's default.
    #[serde(default)]
    #[sqlx(rename = "grp")]
    pub group: Option<String>,
    /// Days from 1 to 365.
    pub days: i64,
}

/// A dataset's SLAs, every one: those left out go back to their default.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SlasBody {
    pub slas: Vec<Sla>,
}

/// A dataset's SLAs and what a form needs to set them.
#[derive(Debug, Serialize, ToSchema)]
pub struct SlasOut {
    /// A queue's days when neither its group nor its dimension has an SLA.
    pub default_days: i64,
    /// The fields holding the `dimension` role in its mapping on `main`;
    /// none without a catalog or a mapping that reads.
    pub dimensions: Vec<String>,
    /// By dimension, its default first, then its groups.
    pub slas: Vec<Sla>,
}

/// Why SLAs are refused: a code the UI's texts translate.
#[derive(Debug, Serialize, ToSchema)]
pub struct SlaRefusal {
    /// `dimension`, `days`, `group`, `duplicate`, `too_many`, `not_found`,
    /// `catalog_not_configured`, `catalog_unavailable` or `forbidden`.
    pub error: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Refused(StatusCode, &'static str);

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        crate::refusal::refuse(self.0, self.1)
    }
}

fn internal(what: impl std::fmt::Display) -> Refused {
    tracing::error!("slas: {what}");
    Refused(StatusCode::INTERNAL_SERVER_ERROR, "internal")
}

const FORBIDDEN: Refused = Refused(StatusCode::FORBIDDEN, "forbidden");
const NO_CATALOG: Refused = Refused(StatusCode::SERVICE_UNAVAILABLE, "catalog_not_configured");
const CATALOG_FAILED: Refused = Refused(StatusCode::SERVICE_UNAVAILABLE, "catalog_unavailable");

fn invalid(code: &'static str) -> Refused {
    Refused(StatusCode::UNPROCESSABLE_ENTITY, code)
}

/// Whether `slas` hold together for a dataset whose dimensions are
/// `dimensions`: the code of the first that does not.
pub fn check(slas: &[Sla], dimensions: &[String]) -> Result<(), &'static str> {
    if slas.len() > MAX_SLAS {
        return Err("too_many");
    }
    let mut seen = BTreeSet::new();
    for sla in slas {
        if !dimensions.contains(&sla.dimension) {
            return Err("dimension");
        }
        if !(1..=MAX_DAYS).contains(&sla.days) {
            return Err("days");
        }
        if sla
            .group
            .as_ref()
            .is_some_and(|g| g.chars().count() > MAX_GROUP)
        {
            return Err("group");
        }
        if !seen.insert((&sla.dimension, &sla.group)) {
            return Err("duplicate");
        }
    }
    Ok(())
}

/// The fields holding the `dimension` role in a mapping's YAML, by name.
pub fn dimensions(mapping: &str) -> Vec<String> {
    Mapping::parse(mapping)
        .map(|m| {
            m.fields
                .iter()
                .filter(|(_, f)| f.value.roles.contains(&FieldRole::Dimension))
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default()
}

impl SlaApi {
    /// The dataset's dimensions on `main`; `None` if the catalog has no
    /// mapping for it.
    async fn dimensions(&self, dataset: &str) -> Result<Option<Vec<String>>, Refused> {
        let catalog = self.catalog.as_ref().ok_or(NO_CATALOG)?;
        match catalog.snapshot(dataset).await {
            Ok(snapshot) => Ok(snapshot.map(|s| dimensions(&s.mapping))),
            Err(e) => {
                tracing::error!("slas: catalog: {e}");
                Err(CATALOG_FAILED)
            }
        }
    }

    async fn out(&self, dataset: &str, dimensions: Vec<String>) -> Result<SlasOut, Refused> {
        let slas = sqlx::query_as::<_, Sla>(
            "SELECT dimension, grp, days::bigint AS days FROM quality.remediation_slas
              WHERE dataset = $1 ORDER BY dimension, grp NULLS FIRST",
        )
        .bind(dataset)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        Ok(SlasOut {
            default_days: DEFAULT_DAYS,
            dimensions,
            slas,
        })
    }
}

/// A dataset's SLAs, with its dimensions and the default.
#[utoipa::path(get, path = "/quality/datasets/{dataset}/queues/slas", operation_id = "quality_queue_slas", tag = "queues",
    params(("dataset" = String, Path)),
    responses((status = 200, body = SlasOut), (status = 401), (status = 403)))]
async fn read(
    State(api): State<SlaApi>,
    Extension(caller): Extension<Caller>,
    Path(dataset): Path<String>,
) -> Result<Json<SlasOut>, Refused> {
    if !caller.can(VIEW, &format!("pipeline/{dataset}")) {
        return Err(FORBIDDEN);
    }
    // Without a catalog the SLAs still read; no dimension to set one on.
    let dimensions = match api.dimensions(&dataset).await {
        Ok(d) => d.unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    Ok(Json(api.out(&dataset, dimensions).await?))
}

/// Replaces a dataset's SLAs, audited; its queues read them at once.
#[utoipa::path(put, path = "/quality/datasets/{dataset}/queues/slas", operation_id = "quality_set_queue_slas", tag = "queues",
    params(("dataset" = String, Path)),
    request_body = SlasBody,
    responses(
        (status = 200, description = "Replaced", body = SlasOut),
        (status = 401, description = "No valid session"),
        (status = 403, description = "May not edit this pipeline"),
        (status = 404, description = "No mapping for the dataset on main", body = SlaRefusal),
        (status = 422, description = "They do not hold together", body = SlaRefusal),
        (status = 503, description = "No catalog to know its dimensions", body = SlaRefusal),
    ))]
async fn replace(
    State(api): State<SlaApi>,
    Extension(caller): Extension<Caller>,
    Path(dataset): Path<String>,
    Json(body): Json<SlasBody>,
) -> Result<Json<SlasOut>, Refused> {
    let resource = format!("pipeline/{dataset}");
    if !caller.can(EDIT, &resource) {
        return Err(FORBIDDEN);
    }
    let dimensions = api
        .dimensions(&dataset)
        .await?
        .ok_or(Refused(StatusCode::NOT_FOUND, "not_found"))?;
    check(&body.slas, &dimensions).map_err(invalid)?;
    let mut tx = api.pool.begin().await.map_err(internal)?;
    sqlx::query("DELETE FROM quality.remediation_slas WHERE dataset = $1")
        .bind(&dataset)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    for sla in &body.slas {
        sqlx::query(
            "INSERT INTO quality.remediation_slas (dataset, dimension, grp, days)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(&dataset)
        .bind(&sla.dimension)
        .bind(&sla.group)
        .bind(i32::try_from(sla.days).map_err(internal)?)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    }
    // Audited before it holds: a change the log cannot record is undone.
    caller
        .audit(AuditEntry {
            action: EDIT,
            resource: format!("{resource}/queues/slas"),
            change: Change::Changed,
        })
        .await
        .map_err(|e| internal(format!("audit: {e}")))?;
    tx.commit().await.map_err(internal)?;
    Ok(Json(api.out(&dataset, dimensions).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sla(dimension: &str, group: Option<&str>, days: i64) -> Sla {
        Sla {
            dimension: dimension.into(),
            group: group.map(Into::into),
            days,
        }
    }

    #[test]
    fn slas_hold_together_only_on_dimensions_with_days_in_bounds_once_each() {
        let dims = vec!["Office".to_string()];
        let ok = [
            sla("Office", None, 30),
            sla("Office", Some("North"), 1),
            sla("Office", Some(""), MAX_DAYS),
        ];
        assert_eq!(check(&ok, &dims), Ok(()));
        assert_eq!(check(&[sla("Amount", None, 30)], &dims), Err("dimension"));
        assert_eq!(check(&[sla("Office", None, 0)], &dims), Err("days"));
        assert_eq!(
            check(&[sla("Office", None, MAX_DAYS + 1)], &dims),
            Err("days")
        );
        let long = "x".repeat(MAX_GROUP + 1);
        assert_eq!(check(&[sla("Office", Some(&long), 5)], &dims), Err("group"));
        let twice = [
            sla("Office", Some("North"), 5),
            sla("Office", Some("North"), 9),
        ];
        assert_eq!(check(&twice, &dims), Err("duplicate"));
        let many = vec![sla("Office", None, 5); MAX_SLAS + 1];
        assert_eq!(check(&many, &dims), Err("too_many"));
    }
}
