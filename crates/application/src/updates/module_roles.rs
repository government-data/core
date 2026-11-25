//! A module's machine roles, as the update agent tells them in `PUT
//! /updater/v1/modules` (quality-sdk decision 0013): each role its manifest
//! declares, the Compose services it runs, how its machines update, the
//! images of those services (pinned by digest) and the files they need
//! (the module's `compose.yml` and what its services mount). A joined
//! machine whose certificate carries a role's name gets that role's images.

use std::collections::BTreeSet;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The most roles a module declares.
pub(super) const MAX_ROLES: usize = 16;
/// The most services, and images, a role lists.
pub(super) const MAX_ROLE_SERVICES: usize = 32;
/// The master's own roles, whose certificates never join.
pub(super) const RESERVED_ROLES: &[&str] = &["app", "updater"];
/// The most a module's files weigh, decoded, each distinct file once.
pub(super) const MAX_MODULE_FILES_BYTES: usize = 256 * 1024;

/// The longest name of a role or a service.
const MAX_NAME: usize = 63;
/// The longest image reference or file path.
const MAX_REFERENCE: usize = 512;

/// How a role's machines update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoleStrategy {
    /// One machine at a time: drain, update, verify.
    One,
    /// All together, in a planned short stop.
    All,
}

/// A role of an installed module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleRole {
    /// The role a joined machine's certificate carries (`OU`).
    pub name: String,
    pub strategy: RoleStrategy,
    /// The module's Compose services this role runs.
    pub services: Vec<String>,
    /// Their images, `<host>/<path>@sha256:<64 hex>`.
    pub images: Vec<String>,
    /// The files its services need; none from an agent that does not send
    /// them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<RoleFile>,
}

/// A file of a role, as the agent read it from the module's package.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleFile {
    /// Relative to the module's folder (`compose.yml`, `config/gate.toml`).
    pub path: String,
    /// The SHA-256 of its content, lowercase hex.
    pub sha256: String,
    /// Its content, standard base64.
    pub content_base64: String,
}

impl std::fmt::Debug for RoleFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoleFile")
            .field("path", &self.path)
            .field("sha256", &self.sha256)
            .finish_non_exhaustive()
    }
}

/// What is wrong with a module's roles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Refusal {
    TooMany,
    BadName(String),
    Reserved(String),
    Repeated(String),
    BadServices(String),
    BadImages(String),
    BadFile(String),
    FilesTooLarge,
}

/// Check a module's roles: names, services, images and files, and the
/// module's files within [`MAX_MODULE_FILES_BYTES`].
pub(super) fn check(roles: &[ModuleRole]) -> Result<(), Refusal> {
    if roles.len() > MAX_ROLES {
        return Err(Refusal::TooMany);
    }
    let mut names = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut bytes = 0usize;
    for role in roles {
        let name = &role.name;
        if !is_role_name(name) {
            return Err(Refusal::BadName(name.clone()));
        }
        if RESERVED_ROLES.contains(&name.as_str()) {
            return Err(Refusal::Reserved(name.clone()));
        }
        if !names.insert(name.as_str()) {
            return Err(Refusal::Repeated(name.clone()));
        }
        if !listed_once(&role.services, is_service) {
            return Err(Refusal::BadServices(name.clone()));
        }
        if !listed_once(&role.images, is_image) {
            return Err(Refusal::BadImages(name.clone()));
        }
        for file in &role.files {
            let size = file_size(file).ok_or_else(|| Refusal::BadFile(name.clone()))?;
            if files.insert((file.path.as_str(), file.sha256.as_str())) {
                bytes += size;
            }
        }
    }
    if bytes > MAX_MODULE_FILES_BYTES {
        return Err(Refusal::FilesTooLarge);
    }
    Ok(())
}

/// 1 to [`MAX_ROLE_SERVICES`] items, each valid and once.
fn listed_once(items: &[String], valid: fn(&str) -> bool) -> bool {
    let unique: BTreeSet<&str> = items.iter().map(String::as_str).collect();
    (1..=MAX_ROLE_SERVICES).contains(&items.len())
        && unique.len() == items.len()
        && items.iter().all(|i| valid(i))
}

/// `[a-z0-9]` then `[a-z0-9_-]`, at most 63.
fn is_role_name(name: &str) -> bool {
    name.len() <= MAX_NAME
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
}

/// A Compose service's name.
fn is_service(name: &str) -> bool {
    name.len() <= MAX_NAME
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// `<host>/<path>@sha256:<64 lowercase hex>`, no scheme, no tag.
fn is_image(image: &str) -> bool {
    let Some((repository, digest)) = image.split_once("@sha256:") else {
        return false;
    };
    let Some((host, path)) = repository.split_once('/') else {
        return false;
    };
    image.len() <= MAX_REFERENCE
        && is_hex64(digest)
        && !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
        && !path.is_empty()
        && path.split('/').all(|s| {
            !s.is_empty()
                && s.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
                })
        })
}

/// The decoded size of a file whose path is relative and stays in the
/// module's folder and whose content matches its SHA-256; `None` otherwise.
fn file_size(file: &RoleFile) -> Option<usize> {
    let path_ok = file.path.len() <= MAX_REFERENCE
        && file.path.split('/').all(|s| {
            !s.is_empty()
                && s != "."
                && s != ".."
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        });
    if !path_ok || !is_hex64(&file.sha256) {
        return None;
    }
    let content = base64::engine::general_purpose::STANDARD
        .decode(&file.content_base64)
        .ok()?;
    let digest = Sha256::digest(&content);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    (hex == file.sha256).then_some(content.len())
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

#[cfg(test)]
#[path = "module_roles_tests.rs"]
pub(crate) mod tests;
