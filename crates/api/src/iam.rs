//! IAM (ADR 0016): the catalog of actions, policies, groups, what each
//! account has, and service accounts.
//!
//! | Route                                | Does                                                  |
//! | ------------------------------------ | ----------------------------------------------------- |
//! | `GET /iam/catalog`                   | the actions (the modules' too) and the kinds          |
//! | `GET /iam/policies`                  | the managed and custom policies, with their use       |
//! | `PUT /iam/policies/{name}`           | create or change a custom one                         |
//! | `DELETE /iam/policies/{name}`        | remove a custom one                                   |
//! | `GET /iam/groups`, `POST /iam/groups` | the groups; create one                               |
//! | `PUT /iam/groups/{id}`, `DELETE`     | change or remove a group                              |
//! | `GET /iam/users`                     | every account's policies and groups                   |
//! | `PUT /iam/users/{id}/policies`       | the policies attached to an account                   |
//! | `GET /iam/service-accounts`, `POST`  | one's own (or all); create one, its token shown once  |
//! | `DELETE /iam/service-accounts/{id}`  | remove one                                            |
//!
//! Which action each needs is decided by [`PlatformService`].

use std::sync::Arc;

use application::{GroupChange, PlatformService};
use auth_gateway::http::rbac::{ActorResolver, ActorSource};
use auth_gateway::SessionService;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use iam::catalog::{ACTIONS, RESOURCE_KINDS};
use iam::Statement;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::platform::{actor, to_api};
use crate::session::{require_session, CurrentSession};

type ApiResult<T> = Result<T, ApiError>;

pub fn routes(platform: PlatformService, sessions: SessionService) -> Router {
    Router::new()
        .route("/iam/catalog", get(catalog))
        .route("/iam/policies", get(list_policies))
        .route(
            "/iam/policies/{name}",
            put(put_policy).delete(delete_policy),
        )
        .route("/iam/groups", get(list_groups).post(create_group))
        .route("/iam/groups/{id}", put(update_group).delete(delete_group))
        .route("/iam/users", get(user_access))
        .route("/iam/users/{id}/policies", put(set_user_policies))
        .route(
            "/iam/service-accounts",
            get(list_service_accounts).post(create_service_account),
        )
        .route(
            "/iam/service-accounts/{id}",
            axum::routing::delete(delete_service_account),
        )
        .with_state(platform)
        .layer(axum::middleware::from_fn_with_state(
            sessions,
            require_session,
        ))
}

/// The actions in display order, with the kind of resource each acts on
/// (`null`: the installation as a whole), then the modules' (quality-sdk
/// decision 0009), each global, with its description in the request's
/// language and its module (`{name, installed}`).
async fn catalog(State(platform): State<PlatformService>) -> ApiResult<Json<Value>> {
    let core = ACTIONS
        .iter()
        .map(|a| json!({ "name": a.name, "resource": a.resource }));
    let modules = platform.module_actions().await.map_err(to_api)?;
    let of_modules = modules.into_iter().map(|a| {
        json!({ "name": a.name, "resource": null, "description": a.description,
                "module": a.module })
    });
    Ok(Json(json!({
        "actions": core.chain(of_modules).collect::<Vec<_>>(),
        "resource_kinds": RESOURCE_KINDS,
    })))
}

async fn list_policies(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
) -> ApiResult<Json<Value>> {
    let actor = actor(&platform, &session).await?;
    let policies = platform.list_policies(&actor).await.map_err(to_api)?;
    Ok(Json(json!(policies)))
}

#[derive(Debug, Deserialize)]
struct PolicyBody {
    #[serde(default)]
    description: String,
    statements: Vec<Statement>,
}

async fn put_policy(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
    Path(name): Path<String>,
    Json(body): Json<PolicyBody>,
) -> ApiResult<StatusCode> {
    let actor = actor(&platform, &session).await?;
    platform
        .put_policy(&actor, &name, &body.description, body.statements)
        .await
        .map_err(to_api)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_policy(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    let actor = actor(&platform, &session).await?;
    platform
        .delete_policy(&actor, &name)
        .await
        .map_err(to_api)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_groups(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
) -> ApiResult<Json<Value>> {
    let actor = actor(&platform, &session).await?;
    let groups = platform.list_groups(&actor).await.map_err(to_api)?;
    Ok(Json(json!(groups)))
}

#[derive(Debug, Deserialize)]
struct GroupBody {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    directory_groups: Vec<String>,
    /// The people added by hand (account ids).
    #[serde(default)]
    members: Vec<String>,
    #[serde(default)]
    policies: Vec<String>,
}

impl From<GroupBody> for GroupChange {
    fn from(b: GroupBody) -> Self {
        Self {
            name: b.name,
            description: b.description,
            directory_groups: b.directory_groups,
            members: b.members,
            policies: b.policies,
        }
    }
}

async fn create_group(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
    Json(body): Json<GroupBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let actor = actor(&platform, &session).await?;
    let id = platform
        .create_group(&actor, &body.into())
        .await
        .map_err(to_api)?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

async fn update_group(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
    Path(id): Path<String>,
    Json(body): Json<GroupBody>,
) -> ApiResult<StatusCode> {
    let actor = actor(&platform, &session).await?;
    platform
        .update_group(&actor, &id, &body.into())
        .await
        .map_err(to_api)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_group(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let actor = actor(&platform, &session).await?;
    platform.delete_group(&actor, &id).await.map_err(to_api)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn user_access(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
) -> ApiResult<Json<Value>> {
    let actor = actor(&platform, &session).await?;
    let access = platform.user_access(&actor).await.map_err(to_api)?;
    Ok(Json(json!(access)))
}

#[derive(Debug, Deserialize)]
struct PoliciesBody {
    policies: Vec<String>,
}

async fn set_user_policies(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
    Path(id): Path<String>,
    Json(body): Json<PoliciesBody>,
) -> ApiResult<StatusCode> {
    let actor = actor(&platform, &session).await?;
    platform
        .set_user_policies(&actor, &id, &body.policies)
        .await
        .map_err(to_api)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_service_accounts(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
) -> ApiResult<Json<Value>> {
    let actor = actor(&platform, &session).await?;
    let list = platform
        .list_service_accounts(&actor)
        .await
        .map_err(to_api)?;
    Ok(Json(json!(list)))
}

#[derive(Debug, Deserialize)]
struct ServiceAccountBody {
    name: String,
    #[serde(default)]
    policies: Vec<String>,
    expires_at: Option<DateTime<Utc>>,
}

/// Create a service account for oneself: the answer carries its token, which
/// is never shown again.
async fn create_service_account(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
    Json(body): Json<ServiceAccountBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let actor = actor(&platform, &session).await?;
    let created = platform
        .create_service_account(&actor, &body.name, &body.policies, body.expires_at)
        .await
        .map_err(to_api)?;
    Ok((StatusCode::CREATED, Json(json!(created))))
}

async fn delete_service_account(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let actor = actor(&platform, &session).await?;
    platform
        .delete_service_account(&actor, &id)
        .await
        .map_err(to_api)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Each session's Actor for `rbac::require_action`: the one from
/// [`PlatformService`], from memory.
struct PlatformActors(PlatformService);

#[async_trait::async_trait]
impl ActorSource for PlatformActors {
    async fn actor(&self, session: &CurrentSession) -> Result<iam::Actor, String> {
        self.0
            .actor(
                &session.admin_id,
                &session.role,
                session.service_account.as_deref(),
            )
            .await
            .map_err(|e| format!("{e:?}"))
    }
}

/// The resolver for the request extensions (`axum::Extension`).
pub fn resolver(platform: PlatformService) -> ActorResolver {
    ActorResolver(Arc::new(PlatformActors(platform)))
}
