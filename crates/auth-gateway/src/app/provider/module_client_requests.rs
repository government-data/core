//! What the update agent sends to register a module's clients (quality-sdk
//! decision 0009), and the clients made from it: checked before anything is
//! stored, never with a secret in a refusal or a log.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::module_clients::{invalid, ModuleClientsError};
use crate::domain::{
    ClientAuth, ClientId, ClientSettings, ModuleClient, OidcClient, RoleClaim, RoleValue,
    SigningAlg,
};

/// The most paths, audiences or capabilities of one kind.
const MAX_ENTRIES: usize = 8;
/// The claims the provider sends already: a role claim cannot take them.
const RESERVED_CLAIMS: [&str; 11] = [
    "sub",
    "iss",
    "aud",
    "exp",
    "iat",
    "auth_time",
    "nonce",
    "email",
    "name",
    "role",
    "policy",
];

/// `PUT /updater/v1/modules/{module}/clients`.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ModuleClientsRequest {
    #[serde(default)]
    pub people: Option<PeopleClient>,
    #[serde(default)]
    pub machine: Option<MachineClient>,
}

/// The client people sign in through.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeopleClient {
    pub redirect_paths: Vec<String>,
    #[serde(default)]
    pub logout_paths: Vec<String>,
    #[serde(default)]
    pub alg: Option<String>,
    #[serde(default = "yes")]
    pub pkce: bool,
    #[serde(default)]
    pub role_claim: Option<RoleClaimRequest>,
    pub secret: String,
    #[serde(default)]
    pub keep_previous: bool,
}

/// A role claim: its name, and each value with the action that gives it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleClaimRequest {
    pub name: String,
    pub values: BTreeMap<String, String>,
}

/// The module's own client, `client_credentials` only.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineClient {
    /// None when its audiences are all it asks for (ruling 94).
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub audiences: Vec<String>,
    /// The capabilities the module requires, from its manifest: what
    /// decides an audience granted only to some (the user's ruling 47 of
    /// 2026-11-25: [`QUALITY_JOBS_AUDIENCE`] needs [`ORCHESTRATION`]).
    #[serde(default)]
    pub requires: Vec<String>,
    pub secret: String,
    #[serde(default)]
    pub keep_previous: bool,
}

// The secrets the agent sends have no business in logs.
impl std::fmt::Debug for ModuleClientsRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModuleClientsRequest")
            .field("people", &self.people)
            .field("machine", &self.machine)
            .finish()
    }
}

impl std::fmt::Debug for PeopleClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeopleClient")
            .field("redirect_paths", &self.redirect_paths)
            .field("role_claim", &self.role_claim)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for RoleClaimRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoleClaimRequest")
            .field("name", &self.name)
            .field("values", &self.values)
            .finish()
    }
}

impl std::fmt::Debug for MachineClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MachineClient")
            .field("capabilities", &self.capabilities)
            .field("audiences", &self.audiences)
            .finish_non_exhaustive()
    }
}

fn yes() -> bool {
    true
}

pub(super) type Wanted = (OidcClient, String, bool);

pub(super) fn people_client(
    module: &str,
    id: &ClientId,
    p: &PeopleClient,
    origin: &str,
) -> Result<Wanted, ModuleClientsError> {
    let redirect_uris = uris(origin, &p.redirect_paths, "redirect_paths")?;
    if redirect_uris.is_empty() {
        return Err(invalid("redirect_paths: at least one"));
    }
    let token_alg = match p.alg.as_deref() {
        None => SigningAlg::EdDSA,
        Some(a) => SigningAlg::parse(a).ok_or_else(|| invalid("alg: EdDSA or ES256"))?,
    };
    let mut side = ModuleClient::of(module);
    side.logout_uris = uris(origin, &p.logout_paths, "logout_paths")?;
    side.role_claim = p.role_claim.as_ref().map(role_claim).transpose()?;
    check_secret(&p.secret)?;
    let client = OidcClient {
        id: id.clone(),
        name: module.to_string(),
        redirect_uris,
        auth: ClientAuth::Public,
        settings: ClientSettings {
            token_alg,
            pkce_required: p.pkce,
            module: Some(side),
            ..ClientSettings::default()
        },
    };
    Ok((client, p.secret.clone(), p.keep_previous))
}

/// The audience of the core's job routes for an orchestrator, its own
/// (not the workers' `quality-api`).
pub(super) const QUALITY_JOBS_AUDIENCE: &str = "quality-jobs";
/// The capability a module must require to get [`QUALITY_JOBS_AUDIENCE`].
pub(super) const ORCHESTRATION: &str = "orchestration";

pub(super) fn machine_client(
    module: &str,
    id: &ClientId,
    m: &MachineClient,
) -> Result<Wanted, ModuleClientsError> {
    let names_ok = |list: &[String]| {
        !list.is_empty()
            && list.len() <= MAX_ENTRIES
            && list.iter().all(|n| {
                !n.is_empty()
                    && n.len() <= 64
                    && n.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            })
    };
    if !m.capabilities.is_empty() && !names_ok(&m.capabilities) {
        return Err(invalid("capabilities: at most 8 names"));
    }
    if !names_ok(&m.audiences) {
        return Err(invalid("audiences: 1 to 8 names"));
    }
    if !m.requires.is_empty() && !names_ok(&m.requires) {
        return Err(invalid("requires: at most 8 names"));
    }
    if m.audiences.iter().any(|a| a == QUALITY_JOBS_AUDIENCE)
        && !m.requires.iter().any(|r| r == ORCHESTRATION)
    {
        return Err(invalid(
            "audiences: quality-jobs is granted only to a module that requires orchestration",
        ));
    }
    check_secret(&m.secret)?;
    let mut side = ModuleClient::of(module);
    side.consumes = m.capabilities.clone();
    let client = OidcClient {
        id: id.clone(),
        name: format!("{module} (machine)"),
        redirect_uris: Vec::new(),
        auth: ClientAuth::Public,
        settings: ClientSettings {
            client_credentials: true,
            audiences: m.audiences.clone(),
            module: Some(side),
            ..ClientSettings::default()
        },
    };
    Ok((client, m.secret.clone(), m.keep_previous))
}

/// The URIs of `paths` under `origin`: each absolute, with no query,
/// fragment, `..` or empty segment but a last one.
fn uris(origin: &str, paths: &[String], field: &str) -> Result<Vec<String>, ModuleClientsError> {
    if paths.len() > MAX_ENTRIES {
        return Err(ModuleClientsError::Invalid(format!("{field}: at most 8")));
    }
    let origin = origin.trim_end_matches('/');
    paths
        .iter()
        .map(|p| {
            let segments: Vec<&str> = p.strip_prefix('/').unwrap_or("").split('/').collect();
            let last = segments.len() - 1;
            let ok = p.starts_with('/')
                && p.len() <= 200
                && p.len() > 1
                && segments
                    .iter()
                    .enumerate()
                    .all(|(i, s)| (i == last || !s.is_empty()) && *s != "." && *s != "..")
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.'));
            if ok {
                Ok(format!("{origin}{p}"))
            } else {
                Err(ModuleClientsError::Invalid(format!(
                    "{field}: an absolute path under the module"
                )))
            }
        })
        .collect()
}

fn role_claim(r: &RoleClaimRequest) -> Result<RoleClaim, ModuleClientsError> {
    let name_ok = (1..=32).contains(&r.name.len())
        && r.name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        && !RESERVED_CLAIMS.contains(&r.name.as_str());
    if !name_ok {
        return Err(invalid(
            "role_claim.name: [a-z_], up to 32, not a claim sent already",
        ));
    }
    if r.values.is_empty() || r.values.len() > 16 {
        return Err(invalid("role_claim.values: 1 to 16"));
    }
    let values = r
        .values
        .iter()
        .map(|(value, action)| {
            let value_ok = (1..=32).contains(&value.len())
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'));
            let action_ok = action.split_once(':').is_some_and(|(p, v)| {
                !p.is_empty() && v.starts_with(|c: char| c.is_ascii_uppercase())
            });
            if value_ok && action_ok {
                Ok(RoleValue {
                    value: value.clone(),
                    action: action.clone(),
                })
            } else {
                Err(invalid(
                    "role_claim.values: a value and the action that gives it",
                ))
            }
        })
        .collect::<Result<_, _>>()?;
    Ok(RoleClaim {
        name: r.name.clone(),
        values,
    })
}

/// A secret the agent made: base64url, 22 to 128 characters (16 bytes and
/// more). Its content is never part of a refusal.
fn check_secret(secret: &str) -> Result<(), ModuleClientsError> {
    let ok = (22..=128).contains(&secret.len())
        && secret
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
    if ok {
        Ok(())
    } else {
        Err(invalid("secret: base64url, 22 to 128 characters"))
    }
}
