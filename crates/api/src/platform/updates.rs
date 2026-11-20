//! The installation's updates.

use super::*;

/// The installation's updates: version, available, progress, policy and
/// history (`null` if it is not updated from here).
pub(super) async fn get_updates(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
) -> ApiResult<Json<Value>> {
    let actor = actor(&platform, &session).await?;
    let view = platform.updates(&actor).await.map_err(to_api)?;
    Ok(Json(json!(view)))
}

pub(super) async fn set_update_policy(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
    Json(policy): Json<application::UpdatePolicy>,
) -> ApiResult<StatusCode> {
    let actor = actor(&platform, &session).await?;
    platform
        .set_update_policy(&actor, policy)
        .await
        .map_err(to_api)?;
    Ok(StatusCode::NO_CONTENT)
}

/// "Update now".
pub(super) async fn request_update(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let actor = actor(&platform, &session).await?;
    let request = platform.request_update(&actor).await.map_err(to_api)?;
    Ok((StatusCode::ACCEPTED, Json(json!(request))))
}
