//! `GET /internal/machines/registry` (quality-sdk decision 0013): a joined,
//! approved machine asks for the registry's pull token and the images of
//! its role, with its certificate on Traefik's internal entry point, as it
//! renews. The role is the certificate's; the images are those of every
//! installed module's role of that name. Answered with `Cache-Control:
//! no-store`; the token is never logged nor audited.

use application::{GrantRefusal, MachinesService, RegistryOutcome, UpdatesService};
use auth_gateway::http::client_cert::{ClientCertificateError, ClientCertificates};
use auth_gateway::MachineCertificate;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde::Serialize;
use serde_json::json;
use utoipa::ToSchema;

use super::origin;
use crate::auth::ClientIp;

/// The path, as the API sees it (Traefik strips `/api`).
pub const REGISTRY_PATH: &str = "/internal/machines/registry";

/// What the route reads: the machines and the installed modules.
#[derive(Clone)]
struct Registry {
    machines: MachinesService,
    updates: UpdatesService,
}

/// Mount the route; `certificates` says whether the client certificate
/// header is trusted.
pub fn routes(
    machines: MachinesService,
    updates: UpdatesService,
    certificates: ClientCertificates,
) -> Router {
    Router::new()
        .route(REGISTRY_PATH, get(registry_handler))
        .with_state(Registry { machines, updates })
        .layer(Extension(certificates))
}

/// The registry's pull token and the role's images.
#[derive(Serialize, ToSchema)]
pub struct RegistryOut {
    /// The registry's DNS name.
    host: String,
    username: String,
    password: String,
    /// RFC 3339.
    expires_at: String,
    /// `<host>/<path>@sha256:<64 hex>`, sorted, each once.
    images: Vec<String>,
    /// Each installed module that declares the role, by name: what the
    /// machine runs of it.
    modules: Vec<RoleModuleOut>,
}

/// A module of the machine's role: its version, the role's services and
/// its files, which the machine puts under `<module>/`.
#[derive(Debug, Serialize, ToSchema)]
pub struct RoleModuleOut {
    name: String,
    version: String,
    services: Vec<String>,
    files: Vec<RoleFileOut>,
}

/// A file of a module's role.
#[derive(Debug, Serialize, ToSchema)]
pub struct RoleFileOut {
    /// Relative to the module's folder.
    path: String,
    /// The SHA-256 of its content, lowercase hex.
    sha256: String,
    /// Its content, standard base64.
    content_base64: String,
}

impl std::fmt::Debug for RegistryOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryOut")
            .field("host", &self.host)
            .field("images", &self.images)
            .finish_non_exhaustive()
    }
}

/// `GET /internal/machines/registry`: the pull token for a machine of a
/// role an installed module declares.
#[utoipa::path(
    get,
    path = "/internal/machines/registry",
    responses(
        (status = 200, description = "The token and the role's images", body = RegistryOut),
        (status = 401, description = "No client certificate"),
        (status = 403, description = "Not an approved machine's certificate in use, or no installed module declares its role"),
        (status = 503, description = "No token held, or it expired: ask again later; or the client certificate header is not trusted"),
    ),
    tag = "machines",
)]
async fn registry_handler(
    State(r): State<Registry>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    presented: Result<MachineCertificate, ClientCertificateError>,
) -> Response {
    let presented = match presented {
        Ok(presented) => presented,
        Err(refused) => return refused.into_response(),
    };
    let origin = origin(ip, &headers);
    let machine = match r.machines.registry_caller(&presented, &origin).await {
        Ok(Some(machine)) => machine,
        Ok(None) => {
            return refusal(
                StatusCode::FORBIDDEN,
                texts::tr!("api.machines.registryRefused"),
            )
        }
        Err(e) => {
            tracing::error!("registry: the machine of a certificate: {e:?}");
            return refusal(
                StatusCode::SERVICE_UNAVAILABLE,
                texts::tr!("api.updater.unavailable"),
            );
        }
    };
    let grant = r.updates.registry_grant(machine.role.as_str()).await;
    let outcome = match &grant {
        Ok(_) => RegistryOutcome::Granted,
        Err(GrantRefusal::NoSuchRole) => RegistryOutcome::NoSuchRole,
        Err(GrantRefusal::NoToken) => RegistryOutcome::NoToken,
        Err(GrantRefusal::Unavailable(e)) => {
            tracing::error!("registry: the installed modules: {e}");
            return refusal(
                StatusCode::SERVICE_UNAVAILABLE,
                texts::tr!("api.updater.unavailable"),
            );
        }
    };
    if let Err(e) = r.machines.registry_called(&machine, outcome, &origin).await {
        tracing::error!("registry: the call could not be audited: {e:?}");
        return refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            texts::tr!("api.updater.unavailable"),
        );
    }
    match grant {
        Ok(grant) => {
            let out = RegistryOut {
                host: grant.token.host,
                username: grant.token.username,
                password: grant.token.password,
                expires_at: grant.token.expires_at.to_rfc3339(),
                images: grant.images,
                modules: grant.modules.into_iter().map(module_out).collect(),
            };
            ([(header::CACHE_CONTROL, "no-store")], Json(out)).into_response()
        }
        Err(GrantRefusal::NoSuchRole) => refusal(
            StatusCode::FORBIDDEN,
            texts::tr!("api.machines.registryNoRole"),
        ),
        Err(_) => refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            texts::tr!("api.machines.registryNoToken"),
        ),
    }
}

fn module_out(m: application::RoleModule) -> RoleModuleOut {
    RoleModuleOut {
        name: m.name,
        version: m.version,
        services: m.services,
        files: m
            .files
            .into_iter()
            .map(|f| RoleFileOut {
                path: f.path,
                sha256: f.sha256,
                content_base64: f.content_base64,
            })
            .collect(),
    }
}

fn refusal(status: StatusCode, why: String) -> Response {
    (status, Json(json!({ "error": why }))).into_response()
}
