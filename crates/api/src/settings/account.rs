//! The shared account and the applications the session holder can go to:
//! `/settings/account` (public) and `/settings/account/apps` (any session).

use std::sync::Arc;

use application::SettingsService;
use auth_gateway::AdminRepository;
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use domain::AdminId;
use serde_json::{json, Value};

use super::ApiResult;
use crate::error::ApiError;
use crate::session::CurrentSession;

/// `GET /settings/account`: `{"url": …}`, the shared account this instance signs
/// in with (`null` without one). The UI signs out through its `/signout` (which
/// also closes that session and goes back to the site) and offers it in the
/// menu: switching application.
pub(super) async fn account_url(State(settings): State<SettingsService>) -> Json<Value> {
    Json(json!({ "url": settings.platform_account() }))
}

/// This installation's applications (those registered with
/// `server oidc-client … --home URL`), in the request extensions.
#[derive(Clone)]
pub struct LocalApps(pub Arc<dyn auth_gateway::OidcClientRegistry>);

impl std::fmt::Debug for LocalApps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LocalApps")
    }
}

/// The ones that sign in with this installation, as entries of the list
/// (`kind: app`).
async fn local_apps(local: Option<&LocalApps>) -> Vec<Value> {
    let Some(LocalApps(registry)) = local else {
        return Vec::new();
    };
    match registry.listed().await {
        Ok(apps) => apps
            .into_iter()
            .filter(|a| a.url.starts_with("https://"))
            .map(|a| {
                json!({
                    "kind": "app",
                    "id": a.id,
                    "name": a.name,
                    "detail": a.detail,
                    "url": a.url,
                    "icon": a.icon.filter(|i| i.starts_with("https://")),
                })
            })
            .collect(),
        Err(e) => {
            tracing::warn!("local apps: {e}");
            Vec::new()
        }
    }
}

/// `GET /settings/account/apps`: where else the session holder can go: this
/// installation's applications (RustFS…) and, with the shared account, by
/// their email, management and their company's instances. If the
/// account does not answer, `502`.
pub(super) async fn account_apps(
    State((settings, admins)): State<(SettingsService, Arc<dyn AdminRepository>)>,
    Extension(session): Extension<CurrentSession>,
    local: Option<Extension<LocalApps>>,
) -> ApiResult<Json<Value>> {
    let mut apps = local_apps(local.as_ref().map(|e| &e.0)).await;
    let email = match admins.find_by_id(&AdminId::new(&session.admin_id)).await {
        Ok(Some(a)) => a.email.to_string(),
        _ => return Ok(Json(json!({ "apps": apps }))),
    };
    match settings.platform_apps(&email).await {
        None => Ok(Json(json!({ "apps": apps }))),
        Some(Ok(v)) => {
            let mut all = v["apps"].as_array().cloned().unwrap_or_default();
            all.append(&mut apps);
            Ok(Json(json!({ "apps": all })))
        }
        Some(Err(e)) => {
            tracing::warn!("account apps: {e}");
            Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "account_unreachable",
                "The common account did not answer.",
            ))
        }
    }
}
