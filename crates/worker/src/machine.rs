//! The machine's certificate as the worker uses it (decision 0066, step 7):
//! the files the node agent keeps in its directory (`QD_MACHINE_DIR`), the
//! machine's name from the certificate, and the HTTP client that presents
//! it to the API's internal entry point and trusts the installation's CA
//! alone.
//!
//! The agent renews every 8 hours and renames the new files over the old
//! ones; the API refuses the old certificate once the new one was presented
//! (point 14), maybe by another process. So the client is not built once:
//! before each request the files are read again, and a client is built
//! anew when they changed. A request already sent finishes with the client
//! it had. Nothing here logs a key or a certificate.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use reqwest::{Certificate, Client, Identity};
use sha2::{Digest, Sha256};
use x509_parser::prelude::{FromDer, X509Certificate};

type Failure = Box<dyn std::error::Error + Send + Sync>;

/// The agent's files (`crates/node-agent`, `store.rs`).
const CA: &str = "ca.crt";
const BUNDLE: &str = "machine.pem";

const TIMEOUT: Duration = Duration::from_secs(30);

/// The node agent's directory.
#[derive(Debug, Clone)]
pub(crate) struct MachineFiles {
    dir: PathBuf,
}

impl MachineFiles {
    pub(crate) fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The installation's CA, the only root trusted for the API and the
    /// queue.
    pub(crate) fn ca(&self) -> PathBuf {
        self.dir.join(CA)
    }

    /// The certificate then its key (PKCS#8), one file renamed whole at
    /// each renewal.
    pub(crate) fn bundle(&self) -> PathBuf {
        self.dir.join(BUNDLE)
    }

    /// Whether the agent has joined: both files are there.
    pub(crate) fn present(&self) -> bool {
        self.ca().is_file() && self.bundle().is_file()
    }

    /// The machine's name: its certificate's `CN`.
    pub(crate) fn machine_name(&self) -> Result<String, Failure> {
        let pem = std::fs::read(self.bundle())?;
        name_of(&pem)
    }

    fn read(&self) -> Result<(Vec<u8>, Vec<u8>), Failure> {
        let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
        Ok((read(&self.bundle())?, read(&self.ca())?))
    }
}

/// The `CN` of the first certificate of a PEM file.
pub(crate) fn name_of(pem: &[u8]) -> Result<String, Failure> {
    let (_, block) = x509_parser::pem::parse_x509_pem(pem)
        .map_err(|_| "the machine's certificate cannot be read")?;
    let (_, cert) = X509Certificate::from_der(&block.contents)
        .map_err(|_| "the machine's certificate cannot be read")?;
    let cn = cert
        .subject()
        .iter_common_name()
        .next()
        .and_then(|cn| cn.as_str().ok())
        .ok_or("the machine's certificate has no CN")?;
    if cn.is_empty() || cn.contains('/') {
        return Err("the machine's certificate has no usable CN".into());
    }
    Ok(cn.to_string())
}

/// A client for the API that presents the machine's certificate, built
/// again whenever the files change.
#[derive(Debug)]
pub(crate) struct MachineHttp {
    files: MachineFiles,
    /// The digest of the files the client was built from, and the client.
    built: Mutex<Option<([u8; 32], Client)>>,
}

impl MachineHttp {
    pub(crate) fn new(files: MachineFiles) -> Self {
        Self {
            files,
            built: Mutex::new(None),
        }
    }

    /// The client for the files as they are now: the last one if they did
    /// not change, a new one if they did.
    pub(crate) fn client(&self) -> Result<Client, Failure> {
        let (bundle, ca) = self.files.read()?;
        let digest: [u8; 32] = Sha256::new()
            .chain_update(&bundle)
            .chain_update([0u8])
            .chain_update(&ca)
            .finalize()
            .into();
        let mut built = self.built.lock().map_err(|_| "the client's lock")?;
        if let Some((seen, client)) = built.as_ref() {
            if *seen == digest {
                return Ok(client.clone());
            }
        }
        let client = build(&bundle, &ca)?;
        if built.is_some() {
            tracing::info!("the machine's certificate changed: a new client for the API");
        }
        *built = Some((digest, client.clone()));
        Ok(client)
    }
}

fn build(bundle: &[u8], ca: &[u8]) -> Result<Client, Failure> {
    let mut builder = Client::builder()
        .timeout(TIMEOUT)
        .https_only(true)
        .tls_built_in_root_certs(false)
        .identity(
            Identity::from_pem(bundle).map_err(|_| "the machine's certificate or key is bad")?,
        );
    for root in Certificate::from_pem_bundle(ca).map_err(|_| "ca.crt cannot be read")? {
        builder = builder.add_root_certificate(root);
    }
    Ok(builder.build()?)
}

#[cfg(test)]
#[path = "machine/tests.rs"]
mod tests;
