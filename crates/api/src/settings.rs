//! HTTP for the system settings: LDAP, outgoing mail, OAuth2/OIDC
//! providers and the platform's own (decision 0079). Only for the global
//! `admin` role.
//!
//! Secrets never leave: in every response the field is empty and next to it a
//! `has_<field>` says whether one is stored. On save, an empty secret keeps
//! the current one. [`SettingsService`] validates, stores and applies; here
//! HTTP is only translated.

use application::{
    LdapSettings, MailSettings, OAuthSettings, PlatformSettings, Section, SettingsError,
    SettingsService, SettingsSource,
};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use auth_gateway::SessionService;

use crate::error::ApiError;
use crate::rbac::require_action;
use crate::session::{require_session, CurrentSession};

mod account;

use account::account_apps;
pub use account::{InstalledModules, LocalApps};

type ApiResult<T> = Result<T, ApiError>;

/// Mount the routes, behind a session and the `admin` role, except
/// `/settings/account/apps`, which is for anyone with a session.
pub fn routes(settings: SettingsService, sessions: SessionService) -> Router {
    let any_session = Router::new()
        .route("/settings/account/apps", get(account_apps))
        .layer(axum::middleware::from_fn_with_state(
            sessions.clone(),
            require_session,
        ));
    Router::new()
        .route("/settings", get(get_all))
        // DELETE goes on each fixed route: a separate `/settings/{section}`
        // would lose against them (axum prefers the fixed route and answers 405).
        .route("/settings/ldap", put(put_ldap).delete(reset_ldap))
        .route("/settings/ldap/test", post(test_ldap))
        .route("/settings/mail", put(put_mail).delete(reset_mail))
        .route("/settings/mail/test", post(test_mail))
        .route("/settings/oauth", put(put_oauth).delete(reset_oauth))
        .route("/settings/oauth/discover", post(discover_oidc))
        .route(
            "/settings/platform",
            put(put_platform).delete(reset_platform),
        )
        .with_state(settings.clone())
        // The current tenant's settings, loaded before using them.
        .layer(axum::middleware::from_fn_with_state(settings, loaded))
        .layer(axum::middleware::from_fn_with_state(
            iam::catalog::CONFIGURE,
            require_action,
        ))
        .layer(axum::middleware::from_fn_with_state(
            sessions,
            require_session,
        ))
        .merge(any_session)
}

async fn loaded(
    State(settings): State<SettingsService>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if let Err(e) = settings.ensure_loaded().await {
        return axum::response::IntoResponse::into_response(to_api(e));
    }
    next.run(request).await
}

fn to_api(e: SettingsError) -> ApiError {
    match e {
        SettingsError::Invalid(fields) => ApiError::validation(fields),
        SettingsError::Unreachable(message) => ApiError::bad_gateway("unreachable", message),
        SettingsError::Internal(detail) => ApiError::internal(format!("settings: {detail}")),
    }
}

#[derive(Debug, Serialize)]
struct SourceOut {
    /// `database`, `environment` o `unset`.
    kind: &'static str,
    updated_at: Option<String>,
    updated_by: Option<String>,
}

fn source(s: &SettingsSource) -> SourceOut {
    match s {
        SettingsSource::Database {
            updated_at,
            updated_by,
        } => SourceOut {
            kind: "database",
            updated_at: Some(updated_at.to_rfc3339()),
            updated_by: updated_by.clone(),
        },
        SettingsSource::Environment => SourceOut {
            kind: "environment",
            updated_at: None,
            updated_by: None,
        },
        SettingsSource::Unset => SourceOut {
            kind: "unset",
            updated_at: None,
            updated_by: None,
        },
    }
}

/// Empty each secret in `object` and leave `has_<field>` next to it.
fn redact(object: &mut Value, fields: &[&str]) {
    let Some(map) = object.as_object_mut() else {
        return;
    };
    for field in fields {
        let set = map
            .get(*field)
            .and_then(Value::as_str)
            .is_some_and(|v| !v.is_empty());
        map.insert((*field).to_string(), json!(""));
        map.insert(format!("has_{field}"), json!(set));
    }
}

fn section_out<T: Serialize>(
    section: &Section<T>,
    secrets: &[&str],
    extend: impl FnOnce(&mut Value),
) -> Value {
    let mut value = serde_json::to_value(&section.value).unwrap_or(Value::Null);
    redact(&mut value, secrets);
    extend(&mut value);
    json!({ "value": value, "source": source(&section.source) })
}

fn ldap_out(s: &Section<LdapSettings>) -> Value {
    section_out(s, &["bind_password"], |_| {})
}

fn mail_out(s: &Section<MailSettings>) -> Value {
    section_out(s, &["smtp_password", "graph_client_secret"], |_| {})
}

fn oauth_out(settings: &SettingsService, s: &Section<OAuthSettings>) -> Value {
    section_out(s, &[], |value| {
        if let Some(providers) = value.get_mut("providers").and_then(Value::as_array_mut) {
            for p in providers {
                redact(p, &["client_secret"]);
                let id = p.get("id").and_then(Value::as_str).unwrap_or_default();
                let uri = settings.oauth_redirect_uri(id);
                p["redirect_uri"] = json!(uri);
            }
        }
    })
}

fn platform_out(s: &Section<PlatformSettings>) -> Value {
    section_out(s, &[], |_| {})
}

/// `GET /settings`: the sections, without secrets.
async fn get_all(State(settings): State<SettingsService>) -> Json<Value> {
    Json(json!({
        "ldap": ldap_out(&settings.ldap()),
        "mail": mail_out(&settings.mail()),
        "oauth": oauth_out(&settings, &settings.oauth()),
        "platform": platform_out(&settings.platform()),
        // The redirect URL of a provider whose id is not written yet.
        "oauth_redirect_template": settings.oauth_redirect_uri("{id}"),
        // How long recovery links last when the mail settings set none.
        "reset_link_default_minutes": settings.reset_link_default_minutes(),
        "encryption_key": settings.key_source(),
    }))
}

async fn put_ldap(
    State(settings): State<SettingsService>,
    Extension(session): Extension<CurrentSession>,
    Json(body): Json<LdapSettings>,
) -> ApiResult<Json<Value>> {
    let saved = settings
        .save_ldap(body, &session.admin_id)
        .await
        .map_err(to_api)?;
    Ok(Json(ldap_out(&saved)))
}

#[derive(Debug, Deserialize)]
struct LdapTestIn {
    settings: LdapSettings,
    /// A person to look up (UPN or email); optional.
    #[serde(default)]
    login: Option<String>,
}

/// `POST /settings/ldap/test`: connect with what the form has.
async fn test_ldap(
    State(settings): State<SettingsService>,
    Json(body): Json<LdapTestIn>,
) -> ApiResult<Json<Value>> {
    let login = body.login.clone();
    let probe = settings
        .test_ldap(body.settings, login.as_deref())
        .await
        .map_err(to_api)?;
    Ok(Json(json!({
        "searched": login.as_deref().is_some_and(|l| !l.trim().is_empty()),
        "user": probe.user.map(|u| json!({
            "dn": u.dn,
            "email": u.email,
            "display_name": u.display_name,
            "groups": u.groups,
        })),
    })))
}

async fn put_mail(
    State(settings): State<SettingsService>,
    Extension(session): Extension<CurrentSession>,
    Json(body): Json<MailSettings>,
) -> ApiResult<Json<Value>> {
    let saved = settings
        .save_mail(body, &session.admin_id)
        .await
        .map_err(to_api)?;
    Ok(Json(mail_out(&saved)))
}

#[derive(Debug, Deserialize)]
struct MailTestIn {
    settings: MailSettings,
    to: String,
}

/// `POST /settings/mail/test`: send a test email with what the form has.
async fn test_mail(
    State(settings): State<SettingsService>,
    Json(body): Json<MailTestIn>,
) -> ApiResult<StatusCode> {
    settings
        .test_mail(body.settings, &body.to)
        .await
        .map_err(to_api)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn put_oauth(
    State(settings): State<SettingsService>,
    Extension(session): Extension<CurrentSession>,
    Json(body): Json<OAuthSettings>,
) -> ApiResult<Json<Value>> {
    let saved = settings
        .save_oauth(body, &session.admin_id)
        .await
        .map_err(to_api)?;
    Ok(Json(oauth_out(&settings, &saved)))
}

#[derive(Debug, Deserialize)]
struct DiscoverIn {
    issuer: String,
}

/// `POST /settings/oauth/discover`: the endpoints an OIDC issuer publishes.
async fn discover_oidc(
    State(settings): State<SettingsService>,
    Json(body): Json<DiscoverIn>,
) -> ApiResult<Json<Value>> {
    let d = settings.discover_oidc(&body.issuer).await.map_err(to_api)?;
    Ok(Json(json!({
        "issuer": d.issuer,
        "auth_endpoint": d.auth_endpoint,
        "token_endpoint": d.token_endpoint,
        "userinfo_endpoint": d.userinfo_endpoint,
    })))
}

/// `DELETE /settings/{ldap,mail,oauth,platform}`: forget what is stored and go back to
/// the environment. Answers all the sections, like `GET /settings`.
async fn reset(
    settings: SettingsService,
    session: &CurrentSession,
    section: &str,
) -> ApiResult<Json<Value>> {
    settings
        .reset(section, &session.admin_id)
        .await
        .map_err(to_api)?;
    Ok(get_all(State(settings)).await)
}

async fn reset_ldap(
    State(settings): State<SettingsService>,
    Extension(session): Extension<CurrentSession>,
) -> ApiResult<Json<Value>> {
    reset(settings, &session, "ldap").await
}

async fn reset_mail(
    State(settings): State<SettingsService>,
    Extension(session): Extension<CurrentSession>,
) -> ApiResult<Json<Value>> {
    reset(settings, &session, "mail").await
}

async fn reset_oauth(
    State(settings): State<SettingsService>,
    Extension(session): Extension<CurrentSession>,
) -> ApiResult<Json<Value>> {
    reset(settings, &session, "oauth").await
}

/// `PUT /settings/platform`: `{"timezone": "<IANA name>"}`.
async fn put_platform(
    State(settings): State<SettingsService>,
    Extension(session): Extension<CurrentSession>,
    Json(body): Json<PlatformSettings>,
) -> ApiResult<Json<Value>> {
    let saved = settings
        .save_platform(body, &session.admin_id)
        .await
        .map_err(to_api)?;
    Ok(Json(platform_out(&saved)))
}

/// `DELETE /settings/platform`: back to UTC.
async fn reset_platform(
    State(settings): State<SettingsService>,
    Extension(session): Extension<CurrentSession>,
) -> ApiResult<Json<Value>> {
    reset(settings, &session, "platform").await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_blanked_and_flagged() {
        let mut v = json!({ "bind_dn": "cn=svc", "bind_password": "s3cret", "other": "" });
        redact(&mut v, &["bind_password", "other"]);
        assert_eq!(v["bind_password"], "");
        assert_eq!(v["has_bind_password"], true);
        assert_eq!(v["has_other"], false);
        assert_eq!(v["bind_dn"], "cn=svc");
    }
}
