//! Global secrets: listed without values, for whoever manages secrets.

use super::*;

pub(super) async fn list_secrets(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
) -> ApiResult<Json<Value>> {
    let actor = actor(&platform, &session).await?;
    let secrets = platform.list_secrets(&actor).await.map_err(to_api)?;
    Ok(Json(json!(secrets)))
}

#[derive(Debug, Deserialize)]
pub(super) struct SecretBody {
    name: String,
    #[serde(default)]
    description: String,
    value: Value,
}

/// Register a global secret.
pub(super) async fn create_secret(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
    Json(body): Json<SecretBody>,
) -> ApiResult<Response> {
    let actor = actor(&platform, &session).await?;
    let secret = platform
        .create_secret(&actor, &body.name, &body.description, &body.value)
        .await
        .map_err(to_api)?;
    Ok((StatusCode::CREATED, Json(json!(secret))).into_response())
}

#[derive(Debug, Deserialize)]
pub(super) struct SecretPatchBody {
    description: Option<String>,
    value: Option<Value>,
}

/// Change a secret's description or value.
pub(super) async fn update_secret(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
    Path(id): Path<String>,
    Json(body): Json<SecretPatchBody>,
) -> ApiResult<Json<Value>> {
    let actor = actor(&platform, &session).await?;
    let secret = platform
        .update_secret(
            &actor,
            &SecretId::new(id),
            body.description.as_deref(),
            body.value.as_ref(),
        )
        .await
        .map_err(to_api)?;
    Ok(Json(json!(secret)))
}

pub(super) async fn delete_secret(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let actor = actor(&platform, &session).await?;
    platform
        .delete_secret(&actor, &SecretId::new(id))
        .await
        .map_err(to_api)?;
    Ok(StatusCode::NO_CONTENT)
}
