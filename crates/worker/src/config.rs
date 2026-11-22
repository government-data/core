//! The worker's settings (see `main.rs`): which of its two modes it runs
//! in, and the API's client built for it.
//!
//! - **Certificate** (`QD_MACHINE_DIR`, decision 0066 step 7): the node
//!   agent's directory. The machine's certificate is its identity for the
//!   API (on the internal entry point) and the queue; it holds no secret,
//!   and a secret given as well stops the start.
//! - **Secret** (no `QD_MACHINE_DIR`): `QD_WORKER_CLIENT_SECRET`, for the
//!   development stack only, until it moves to certificates (step 8).

use std::time::Duration;

use crate::api::{self, Api, Auth};
use crate::machine::{MachineFiles, MachineHttp};

type Failure = Box<dyn std::error::Error + Send + Sync>;

/// How often a worker whose agent has not joined yet looks again.
const JOIN_WAIT: Duration = Duration::from_secs(5);

/// A setting, or the contents of the file `<NAME>_FILE` names.
pub(crate) fn value(name: &str) -> Result<String, Failure> {
    optional(name)?.ok_or_else(|| format!("{name} (or {name}_FILE) is needed").into())
}

/// A setting, its file's contents, or nothing.
pub(crate) fn optional(name: &str) -> Result<Option<String>, Failure> {
    if let Some(v) = env(name) {
        return Ok(Some(v));
    }
    let Some(file) = env(&format!("{name}_FILE")) else {
        return Ok(None);
    };
    Ok(Some(
        std::fs::read_to_string(&file)
            .map_err(|e| format!("{name}_FILE {file}: {e}"))?
            .trim()
            .to_string(),
    ))
}

pub(crate) fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// DuckDB's extension directory, which the worker's image fills when it is
/// built (decision 0077 point 8): `QD_DUCKDB_EXTENSIONS`, else DuckDB's own
/// under `$HOME` (`~/.duckdb/extensions`, where the image puts them).
pub(crate) fn extension_directory() -> Option<String> {
    extension_directory_of(env("QD_DUCKDB_EXTENSIONS"), env("HOME"))
}

fn extension_directory_of(set: Option<String>, home: Option<String>) -> Option<String> {
    set.or_else(|| home.map(|h| format!("{}/.duckdb/extensions", h.trim_end_matches('/'))))
}

/// How the worker proves who it is. The secret never shows in `Debug`.
pub(crate) enum Mode {
    Secret(String),
    Certificate(MachineFiles),
}

impl std::fmt::Debug for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Secret(_) => f.write_str("Secret(..)"),
            Self::Certificate(files) => f.debug_tuple("Certificate").field(files).finish(),
        }
    }
}

impl Mode {
    pub(crate) fn from_env() -> Result<Self, Failure> {
        let secret_set = env("QD_WORKER_CLIENT_SECRET").is_some()
            || env("QD_WORKER_CLIENT_SECRET_FILE").is_some();
        Self::of(env("QD_MACHINE_DIR"), secret_set, || {
            value("QD_WORKER_CLIENT_SECRET")
        })
    }

    /// The mode for a machine directory, if any, and whether a secret is
    /// set (`secret` reads it).
    fn of(
        dir: Option<String>,
        secret_set: bool,
        secret: impl FnOnce() -> Result<String, Failure>,
    ) -> Result<Self, Failure> {
        match (dir, secret_set) {
            (Some(_), true) => Err("QD_WORKER_CLIENT_SECRET is set with QD_MACHINE_DIR: with \
                                    the machine's certificate the worker holds no secret"
                .into()),
            (Some(dir), false) => Ok(Self::Certificate(MachineFiles::new(dir))),
            (None, _) => Ok(Self::Secret(secret()?)),
        }
    }

    /// The machine's files, with a certificate.
    pub(crate) fn files(&self) -> Option<&MachineFiles> {
        match self {
            Self::Certificate(files) => Some(files),
            Self::Secret(_) => None,
        }
    }

    /// The API's client for this mode (with a certificate, once the agent's
    /// files are there: [`wait_for_join`]).
    pub(crate) fn api(self, http: reqwest::Client, url: &str) -> Result<Api, Failure> {
        let auth = match self {
            Self::Secret(secret) => Auth::Secret(secret),
            Self::Certificate(files) => {
                let machine = files.machine_name()?;
                Auth::Certificate {
                    http: MachineHttp::new(files),
                    machine,
                }
            }
        };
        let id = value("QD_WORKER_CLIENT_ID")?;
        Ok(Api::new(http, url, api::Client { id, auth }))
    }
}

/// Until the node agent has joined and written its files.
pub(crate) async fn wait_for_join(files: &MachineFiles) {
    if files.present() {
        return;
    }
    tracing::info!("waiting for the node agent's certificate in QD_MACHINE_DIR");
    while !files.present() {
        tokio::time::sleep(JOIN_WAIT).await;
    }
}

/// Until the lake's proxy at `url` (`QD_LAKE_URL`) takes connections: it
/// starts beside the worker, and a job taken before it would fail.
pub(crate) async fn wait_for_lake(url: &str) -> Result<(), Failure> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("QD_LAKE_URL: {e}"))?;
    let (Some(host), Some(port)) = (parsed.host_str(), parsed.port_or_known_default()) else {
        return Err("QD_LAKE_URL: no host".into());
    };
    let mut told = false;
    while tokio::net::TcpStream::connect((host, port)).await.is_err() {
        if !told {
            tracing::info!("waiting for the lake's proxy at QD_LAKE_URL");
            told = true;
        }
        tokio::time::sleep(JOIN_WAIT).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{extension_directory_of, Mode};

    #[test]
    fn the_extension_directory_is_the_images_or_duckdbs_own() {
        let some = |s: &str| Some(s.to_string());
        assert_eq!(
            extension_directory_of(some("/ext"), some("/home/nonroot")),
            some("/ext")
        );
        assert_eq!(
            extension_directory_of(None, some("/home/nonroot/")),
            some("/home/nonroot/.duckdb/extensions")
        );
        assert_eq!(extension_directory_of(None, None), None);
    }

    fn secret() -> Result<String, super::Failure> {
        Ok("s3cret".into())
    }

    #[test]
    fn a_machine_directory_is_the_certificate_mode_and_takes_no_secret() {
        let mode = Mode::of(Some("/var/lib/qd-node".into()), false, || {
            panic!("no secret is read with a certificate")
        })
        .unwrap();
        let files = mode.files().unwrap();
        assert_eq!(
            files.bundle().to_str(),
            Some("/var/lib/qd-node/machine.pem")
        );
        assert_eq!(files.ca().to_str(), Some("/var/lib/qd-node/ca.crt"));
        let both = Mode::of(Some("/var/lib/qd-node".into()), true, secret).unwrap_err();
        assert!(
            both.to_string().contains("QD_WORKER_CLIENT_SECRET"),
            "{both}"
        );
    }

    #[test]
    fn without_a_machine_directory_the_secret_is_needed() {
        let mode = Mode::of(None, true, secret).unwrap();
        assert!(matches!(&mode, Mode::Secret(s) if s == "s3cret"));
        assert!(!format!("{mode:?}").contains("s3cret"));
        let none = Mode::of(None, false, || {
            Err("QD_WORKER_CLIENT_SECRET is needed".into())
        });
        assert!(none.is_err());
    }
}
