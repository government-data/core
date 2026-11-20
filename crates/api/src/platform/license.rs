//! The installation's license and updates.

use super::*;

/// The license and how much of the plan is used (`null` if the installation
/// has no license).
pub(super) async fn get_license(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
) -> ApiResult<Json<Value>> {
    let actor = actor(&platform, &session).await?;
    let license = platform.license(&actor).await.map_err(to_api)?;
    Ok(Json(match license {
        Some((view, usage)) => json!({ "license": view, "usage": usage }),
        None => Value::Null,
    }))
}

#[derive(Deserialize)]
pub(super) struct LicenseKeyBody {
    key: String,
}

/// Change the activation key (activated right away with the cloud).
pub(super) async fn set_license_key(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
    Json(body): Json<LicenseKeyBody>,
) -> ApiResult<StatusCode> {
    let actor = actor(&platform, &session).await?;
    platform
        .set_license_key(&actor, &body.key)
        .await
        .map_err(to_api)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Renew the permit with the cloud now.
pub(super) async fn refresh_license(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
) -> ApiResult<StatusCode> {
    let actor = actor(&platform, &session).await?;
    platform.refresh_license(&actor).await.map_err(to_api)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Payments and renewal, as the licensing cloud reports them.
pub(super) async fn license_billing(
    State(platform): State<PlatformService>,
    Extension(session): Extension<CurrentSession>,
) -> ApiResult<Json<Value>> {
    let actor = actor(&platform, &session).await?;
    let view = platform.license_billing(&actor).await.map_err(to_api)?;
    Ok(Json(json!(view)))
}

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
