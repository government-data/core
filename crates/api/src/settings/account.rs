//! The shared account and the applications the session holder can go to:
//! `/settings/account` (public) and `/settings/account/apps` (any session).

use std::collections::BTreeSet;
use std::sync::Arc;

use application::{InstalledModule, SettingsService, UpdatesService};
use auth_gateway::http::rbac::ActorResolver;
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

/// The installed modules as the update agent tells them (quality-sdk
/// decision 0002): those with a menu entry are offered in the list, in the
/// request's language. In the request extensions.
#[derive(Clone)]
pub struct InstalledModules(pub UpdatesService);

impl std::fmt::Debug for InstalledModules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InstalledModules")
    }
}

/// The installed modules with a menu entry the session's person may see,
/// as entries of the list (`kind: module`): their label and page in the
/// request's language, opened on this site.
async fn module_apps(modules: Option<&InstalledModules>, actor: Option<&iam::Actor>) -> Vec<Value> {
    let Some(InstalledModules(updates)) = modules else {
        return Vec::new();
    };
    match updates.installed_modules().await {
        Ok(modules) => module_entries(modules, texts::current().code(), actor),
        Err(e) => {
            tracing::warn!("installed modules: {e}");
            Vec::new()
        }
    }
}

/// The entries of the modules that have a menu entry, in `language`. An
/// entry with `requires` is shown only to an `actor` who holds that action
/// on some resource (quality-sdk decision 0009); an action neither the core
/// nor an installed module declares hides it, as does no actor. It hides a
/// link and grants nothing.
fn module_entries(
    modules: Vec<InstalledModule>,
    language: &str,
    actor: Option<&iam::Actor>,
) -> Vec<Value> {
    let declared: BTreeSet<String> = modules
        .iter()
        .flat_map(|m| m.permissions.iter().flat_map(|p| &p.actions))
        .map(|a| a.name.clone())
        .collect();
    let holds = |action: &str| {
        let known = iam::catalog::action(action).is_some() || declared.contains(action);
        known && actor.is_some_and(|a| a.can_some(action))
    };
    modules
        .into_iter()
        .filter_map(|m| {
            let menu = m.menu?;
            if menu.requires.as_deref().is_some_and(|r| !holds(r)) {
                return None;
            }
            Some(json!({
                "kind": "module",
                "id": m.name,
                "name": menu.label.get(language),
                "detail": "",
                "url": menu.path.get(language),
            }))
        })
        .collect()
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
/// installation's modules with a menu entry (the user guide…) and
/// applications (RustFS…) and, with the shared account, by
/// their email, management and their company's instances. If the
/// account does not answer, `502`.
pub(super) async fn account_apps(
    State((settings, admins)): State<(SettingsService, Arc<dyn AdminRepository>)>,
    Extension(session): Extension<CurrentSession>,
    local: Option<Extension<LocalApps>>,
    modules: Option<Extension<InstalledModules>>,
    resolver: Option<Extension<ActorResolver>>,
) -> ApiResult<Json<Value>> {
    let actor = match resolver {
        Some(Extension(resolver)) => resolver.0.actor(&session).await.ok(),
        None => None,
    };
    let mut apps = module_apps(modules.as_ref().map(|e| &e.0), actor.as_ref()).await;
    apps.append(&mut local_apps(local.as_ref().map(|e| &e.0)).await);
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

#[cfg(test)]
mod tests {
    use application::{InLanguages, ModuleAction, ModuleMenu, ModulePermissions};

    use super::*;

    fn module(name: &str, menu: Option<ModuleMenu>) -> InstalledModule {
        InstalledModule {
            name: name.into(),
            version: "1.0.0".into(),
            drain_path: None,
            route_path: menu.as_ref().map(|_| format!("/{name}")),
            menu,
            permissions: None,
            provides: Vec::new(),
        }
    }

    #[test]
    fn only_modules_with_a_menu_entry_are_offered_in_the_language() {
        let guide = ModuleMenu {
            label: InLanguages {
                en: "User guide".into(),
                es: "Guía de usuario".into(),
            },
            path: InLanguages {
                en: "/docs/".into(),
                es: "/docs/es/".into(),
            },
            requires: None,
        };
        let modules = vec![module("quality-data", None), module("docs", Some(guide))];
        assert_eq!(
            module_entries(modules.clone(), "es", None),
            [
                json!({ "kind": "module", "id": "docs", "name": "Guía de usuario",
                     "detail": "", "url": "/docs/es/" })
            ]
        );
        assert_eq!(module_entries(modules, "en", None)[0]["url"], "/docs/");
        assert!(module_entries(vec![module("quality-data", None)], "en", None).is_empty());
    }

    fn analytics(requires: &str) -> InstalledModule {
        let text = |t: &str| InLanguages {
            en: t.into(),
            es: t.into(),
        };
        let mut m = module(
            "bi-superset",
            Some(ModuleMenu {
                label: text("Analytics"),
                path: text("/bi-superset/"),
                requires: Some(requires.into()),
            }),
        );
        m.permissions = Some(ModulePermissions {
            actions: vec![ModuleAction {
                name: "bi-superset:Author".into(),
                description: text("Create charts"),
            }],
            policies: vec![],
        });
        m
    }

    fn holding(actions: &[&str]) -> iam::Actor {
        let policy = iam::Policy {
            name: "p".into(),
            description: String::new(),
            statements: vec![iam::Statement::allow(actions, &["*"])],
            managed: false,
        };
        iam::Actor::account("ana", false, iam::Permissions::from_policies(&[policy]))
    }

    #[test]
    fn an_entry_that_requires_an_action_is_shown_only_to_who_holds_it() {
        let shown = |requires: &str, actor: Option<&iam::Actor>| {
            !module_entries(vec![analytics(requires)], "en", actor).is_empty()
        };
        let author = holding(&["bi-superset:Author"]);
        let viewer = holding(&["quality:ViewResults"]);
        assert!(shown("bi-superset:Author", Some(&author)));
        assert!(!shown("bi-superset:Author", Some(&viewer)));
        assert!(!shown("bi-superset:Author", None), "no actor, hidden");
        assert!(shown("quality:ViewResults", Some(&viewer)), "a core action");
        let everything = holding(&["*"]);
        assert!(shown("bi-superset:Author", Some(&everything)));
        assert!(
            !shown("bi-superset:Fly", Some(&everything)),
            "unknown hides"
        );
        let admin = iam::Actor::account("root", true, iam::Permissions::default());
        assert!(shown("bi-superset:Author", Some(&admin)));
        assert!(!shown("other:Author", Some(&admin)), "unknown hides");
    }
}
