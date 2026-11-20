//! What a new machine is installed with (decision 0029; 0066 step 11 and
//! the user's ruling of 2026-11-13): one link fetched with `curl`, checked
//! by hashes, whose script brings each server what it needs. The script and
//! the CA are anyone's (the command checks both by their hashes); the role's
//! compose file, which carries the installation's details, and the
//! entrypoints it names are given only to whoever presents a join token that is unused and not expired, and the
//! token is not used up by it: the node agent uses it to join.
//!
//! The command Settings shows is built here, from the same files and hashes
//! the routes serve, so the screen and the script cannot disagree.

use std::sync::Arc;

use auth_gateway::{
    is_join_token, InstallFile, InstallFiles, JoinGrant, JoinTokenCheck, MachineRole,
};

use super::{Drives, IssuedJoinToken, MachinesService};

/// The installation's files, its join tokens and where it is.
#[derive(Clone)]
pub(super) struct Install {
    files: Arc<dyn InstallFiles>,
    tokens: Arc<dyn JoinTokenCheck>,
    /// `https://<domain>`, without a trailing slash; `None` when the
    /// public address is not HTTPS or cannot be quoted in a command: no
    /// command is made then, as the token never travels unencrypted.
    public_url: Option<String>,
    /// Where a joined machine's Alloy pushes its metrics (decision 0071,
    /// point 14), given to `join.sh` as `--metrics-url`; `None` when unset
    /// or not quotable HTTPS: the command then has none and the machine
    /// runs no Alloy.
    metrics_url: Option<String>,
}

/// Why a role's compose file was not given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallFileError {
    /// No token, or one unknown, used, expired or for another role: one
    /// answer for all.
    Refused,
    /// The role has no compose file yet.
    NotFound,
    /// This API serves no installation files.
    Unavailable,
    Backend(String),
}

/// What Settings shows to add a machine: the command, ready to copy, and
/// the hashes in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinCommand {
    /// Where the command downloads `join.sh` from.
    pub script_url: String,
    pub script_sha256: String,
    /// The CA's fingerprint, the SHA-256 of its DER.
    pub ca_sha256: String,
    /// The command, holding the token.
    pub command: String,
}

/// The file name the command saves the script as.
const SCRIPT_FILE: &str = "qd-join.sh";

impl MachinesService {
    /// With the files a machine is installed with, the check of a join
    /// token that does not use it, and the installation's public address
    /// (`APP_PUBLIC_URL`, where the API is under `/api`).
    pub fn with_install(
        mut self,
        files: Arc<dyn InstallFiles>,
        tokens: Arc<dyn JoinTokenCheck>,
        public_url: &str,
    ) -> Self {
        self.install = Some(Install {
            files,
            tokens,
            public_url: command_url(public_url),
            metrics_url: None,
        });
        self
    }

    /// With where the machines it installs push their metrics (Mimir's
    /// remote write, `https://<observability server>:9009/api/v1/push`);
    /// ignored before [`Self::with_install`], and when not quotable HTTPS.
    pub fn with_metrics_push_url(mut self, url: Option<&str>) -> Self {
        if let Some(install) = self.install.as_mut() {
            install.metrics_url = url.and_then(command_url);
        }
        self
    }

    /// `join.sh` and its SHA-256: anyone's.
    pub fn join_script(&self) -> Option<InstallFile> {
        self.install.as_ref().map(|i| i.files.script())
    }

    /// The installation's CA and its fingerprint: anyone's.
    pub fn install_ca(&self) -> Option<InstallFile> {
        self.install.as_ref().and_then(|i| i.files.ca())
    }

    /// The compose file of `role`, for whoever presents a join token of that
    /// role that is unused and not expired. The token is not used up.
    pub async fn role_compose(
        &self,
        token: &str,
        role: MachineRole,
    ) -> Result<InstallFile, InstallFileError> {
        let install = self.granted(token, role).await?;
        install
            .files
            .compose(role)
            .ok_or(InstallFileError::NotFound)
    }

    /// Another file the compose file of `role` names (an entrypoint), on
    /// the same terms as the compose file.
    pub async fn role_file(
        &self,
        token: &str,
        role: MachineRole,
        name: &str,
    ) -> Result<InstallFile, InstallFileError> {
        let install = self.granted(token, role).await?;
        install
            .files
            .file(role, name)
            .ok_or(InstallFileError::NotFound)
    }

    /// The installation's files, if `token` is a usable join token of
    /// `role`; one refusal for every other token.
    pub(super) async fn granted(
        &self,
        token: &str,
        role: MachineRole,
    ) -> Result<&Install, InstallFileError> {
        self.grant(token, role).await.map(|(install, _)| install)
    }

    /// The installation's files and what `token` is for, if it is a usable
    /// join token of `role`; one refusal for every other token.
    pub(super) async fn grant(
        &self,
        token: &str,
        role: MachineRole,
    ) -> Result<(&Install, JoinGrant), InstallFileError> {
        let Some(install) = &self.install else {
            return Err(InstallFileError::Unavailable);
        };
        if !is_join_token(token) {
            return Err(InstallFileError::Refused);
        }
        let hash = self.mint.hash(token);
        let grant = install
            .tokens
            .usable(&hash, self.clock.now())
            .await
            .map_err(|e| InstallFileError::Backend(e.to_string()))?;
        match grant {
            Some(grant) if grant.role == role => Ok((install, grant)),
            _ => Err(InstallFileError::Refused),
        }
    }

    /// The command that installs the machine `issued` is for, with every
    /// hash in it and a `--drive` for each of `drives` (checked for its
    /// role by [`Drives::parse`]); `None` when there is nothing to install
    /// it with: a role without a compose file, no CA, or no HTTPS public
    /// address.
    pub fn join_command(&self, issued: &IssuedJoinToken, drives: &Drives) -> Option<JoinCommand> {
        let install = self.install.as_ref()?;
        let url = install.public_url.as_deref()?;
        install.files.compose(issued.role)?;
        let ca = install.files.ca()?;
        let script = install.files.script();
        let script_url = format!("{url}/api/install/join.sh");
        let pool = issued
            .pool
            .as_deref()
            .map(|p| format!(" --pool '{p}'"))
            .unwrap_or_default();
        // A pool node's token names it; the server joins under that name.
        let pool = match issued.node.as_deref() {
            Some(node) => format!("{pool} --name '{node}'"),
            None => pool,
        };
        let metrics = install
            .metrics_url
            .as_deref()
            .map(|m| format!(" \\\n\x20    --metrics-url '{m}'"))
            .unwrap_or_default();
        let command = format!(
            "curl -fsS --proto '=https' -o {SCRIPT_FILE} '{script_url}' \\\n\
             && echo '{sha}  {SCRIPT_FILE}' | sha256sum -c - \\\n\
             && sh {SCRIPT_FILE} --url '{url}' --role {role}{pool}{drives}{metrics} \\\n\
             \x20    --ca-sha256 '{ca}' --token '{token}'",
            sha = script.sha256,
            role = issued.role.as_str(),
            drives = drives.arguments(),
            ca = ca.sha256,
            token = issued.token,
        );
        Some(JoinCommand {
            script_url,
            script_sha256: script.sha256,
            ca_sha256: ca.sha256,
            command,
        })
    }
}

/// `url` without a trailing slash, if it is HTTPS and every character can
/// stand in single quotes in a shell command.
fn command_url(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let rest = url.strip_prefix("https://")?;
    let safe = |c: char| c.is_ascii_alphanumeric() || ".-:/_~".contains(c);
    (!rest.is_empty() && rest.chars().all(safe)).then(|| url.to_string())
}
