//! The agent's settings, from the environment (the names are in `main.rs`).
//! What only the join needs is optional here and asked for when joining.

use std::path::PathBuf;
use std::time::Duration;

use crate::plan::Mode;

/// Every setting, read once at start.
#[derive(Debug, Clone)]
pub struct Settings {
    pub dir: PathBuf,
    /// The API on the public entry point, for the join.
    pub public_url: Option<String>,
    /// The API on the internal entry point, for renewing.
    pub internal_url: String,
    pub name: Option<String>,
    pub token: Option<JoinToken>,
    /// The CA's SHA-256 the join's answer must have (0029's `--ca-sha256`).
    pub ca_sha256: Option<String>,
    /// A CA trusted for the join's TLS, besides the public roots.
    pub ca_file: Option<PathBuf>,
    pub renew_every: Duration,
    /// Run after a new certificate is in place.
    pub after_renew: Option<Vec<String>>,
    pub mode: Mode,
    /// In held mode, exits 0 when the next certificate may be taken.
    pub held_ready: Option<Vec<String>>,
}

/// The join token, given once.
#[derive(Clone)]
pub enum JoinToken {
    Value(String),
    /// In a file, deleted once the machine joined.
    File(PathBuf),
}

impl std::fmt::Debug for JoinToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Value(_) => f.write_str("JoinToken::Value(..)"),
            Self::File(path) => f.debug_tuple("JoinToken::File").field(path).finish(),
        }
    }
}

impl JoinToken {
    pub fn read(&self) -> Result<String, String> {
        let token = match self {
            Self::Value(v) => v.clone(),
            Self::File(path) => std::fs::read_to_string(path)
                .map_err(|e| format!("QD_JOIN_TOKEN_FILE {}: {e}", path.display()))?,
        };
        Some(token.trim().to_string())
            .filter(|t| !t.is_empty())
            .ok_or_else(|| "the join token is empty".to_string())
    }
}

const RENEW_EVERY: Duration = Duration::from_secs(8 * 3600);
const HELD_AFTER: Duration = Duration::from_secs(24 * 3600);
const HELD_ASK_EVERY: Duration = Duration::from_secs(300);

impl Settings {
    /// The settings `get` gives (the environment, in the binary).
    pub fn from(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let get = |name: &str| {
            get(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let required = |name: &str| get(name).ok_or_else(|| format!("{name} is needed"));
        let duration = |name: &str, default| match get(name) {
            Some(v) => parse_duration(&v).map_err(|e| format!("{name}: {e}")),
            None => Ok(default),
        };
        let command = |name: &str| {
            get(name)
                .map(|v| parse_command(&v).map_err(|e| format!("{name}: {e}")))
                .transpose()
        };
        let token = match (get("QD_JOIN_TOKEN_FILE"), get("QD_JOIN_TOKEN")) {
            (Some(file), _) => Some(JoinToken::File(file.into())),
            (None, Some(value)) => Some(JoinToken::Value(value)),
            (None, None) => None,
        };
        let mode = match get("QD_RENEW_MODE").as_deref() {
            None | Some("rolling") => Mode::Rolling,
            Some("held") => Mode::Held {
                after: duration("QD_HELD_AFTER", HELD_AFTER)?,
                ask_every: duration("QD_HELD_ASK_EVERY", HELD_ASK_EVERY)?,
            },
            Some(other) => return Err(format!("QD_RENEW_MODE: rolling or held, not {other}")),
        };
        let held_ready = command("QD_HELD_READY")?;
        if matches!(mode, Mode::Held { .. }) && held_ready.is_none() {
            return Err("QD_HELD_READY is needed with QD_RENEW_MODE=held".into());
        }
        Ok(Self {
            dir: required("QD_AGENT_DIR")?.into(),
            public_url: get("QD_PUBLIC_URL"),
            internal_url: required("QD_INTERNAL_URL")?,
            name: get("QD_MACHINE_NAME"),
            token,
            ca_sha256: get("QD_CA_SHA256"),
            ca_file: get("QD_CA_FILE").map(PathBuf::from),
            renew_every: duration("QD_RENEW_EVERY", RENEW_EVERY)?,
            after_renew: command("QD_AFTER_RENEW")?,
            mode,
            held_ready,
        })
    }
}

/// The settings of the follower that restarts Dolt when it is free (0066,
/// step 10), on the application VM's agent, which runs rolling. Only with
/// `QD_DOLT_RESTART`.
#[derive(Debug, Clone)]
pub struct DoltSettings {
    /// `QD_DOLT_RESTART`: restarts Dolt (`docker restart <container>`).
    pub restart: Vec<String>,
    /// `QD_DOLT_ADDR`: Dolt's `host:port`, asked until it takes a connection.
    pub addr: String,
    /// `QD_DOLT_WAIT` (2 minutes): how long Dolt may take to answer again.
    pub wait: Duration,
    /// `QD_HELD_AFTER` (24 hours): how old Dolt's certificate may get first.
    pub after: Duration,
    /// `QD_HELD_ASK_EVERY` (5 minutes): how often the follower looks.
    pub every: Duration,
}

impl DoltSettings {
    /// `None` without `QD_DOLT_RESTART`.
    pub fn from(get: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, String> {
        let get = |name: &str| {
            get(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let Some(restart) = get("QD_DOLT_RESTART") else {
            return Ok(None);
        };
        let duration = |name: &str, default: u64| match get(name) {
            Some(v) => parse_duration(&v).map_err(|e| format!("{name}: {e}")),
            None => Ok(Duration::from_secs(default)),
        };
        Ok(Some(Self {
            restart: parse_command(&restart).map_err(|e| format!("QD_DOLT_RESTART: {e}"))?,
            addr: get("QD_DOLT_ADDR").ok_or("QD_DOLT_ADDR is needed with QD_DOLT_RESTART")?,
            wait: duration("QD_DOLT_WAIT", 120)?,
            after: duration("QD_HELD_AFTER", 24 * 3600)?,
            every: duration("QD_HELD_ASK_EVERY", 300)?,
        }))
    }
}

/// `90`, `90s`, `5m`, `8h` or `1d`; never zero.
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let (number, unit) = match text.char_indices().last() {
        Some((i, c)) if c.is_ascii_alphabetic() => (&text[..i], c),
        _ => (text, 's'),
    };
    let seconds = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        _ => return Err(format!("{text}: a number of s, m, h or d")),
    };
    let n: u64 = number
        .parse()
        .map_err(|_| format!("{text}: a number of s, m, h or d"))?;
    match n.checked_mul(seconds) {
        Some(s) if s > 0 => Ok(Duration::from_secs(s)),
        _ => Err(format!("{text}: more than zero")),
    }
}

/// A command run without a shell: a JSON array of strings (`["pg_ctl",
/// "reload"]`), or words split at whitespace (`pg_ctl reload`).
pub fn parse_command(text: &str) -> Result<Vec<String>, String> {
    let text = text.trim();
    let argv: Vec<String> = if text.starts_with('[') {
        serde_json::from_str(text).map_err(|_| "a JSON array of strings".to_string())?
    } else {
        text.split_whitespace().map(str::to_string).collect()
    };
    if argv.first().is_none_or(|program| program.is_empty()) {
        return Err("a command is empty".into());
    }
    Ok(argv)
}

#[cfg(test)]
mod tests;
