//! The API's HTTP client for the lake (RustFS: its S3 and admin APIs, its
//! Iceberg catalog, its STS). It trusts `LAKE_CA_FILE` too if it is set, and
//! presents the application VM's machine certificate if
//! `LAKE_CLIENT_CERT_FILE` names it (decision 0066, point 10: the lake is
//! reached only with a machine's certificate, through its gate). That file
//! is the node agent's `machine.pem`, the certificate then the key, renamed
//! whole at each renewal: it is read again whenever it changes, so the API
//! presents the renewed certificate without a restart. If it cannot be read
//! then, the client in use stays and the error is logged, never the file.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

/// The lake's HTTP client, as its users hold it: one made by the caller, or
/// one that follows the machine's certificate file.
#[derive(Clone)]
pub struct LakeClient(Arc<Inner>);

enum Inner {
    Fixed(reqwest::Client),
    Following(Following),
}

/// A client rebuilt when the certificate's file changes.
struct Following {
    timeout: Duration,
    /// The CA to trust besides the system's (PEM), read once.
    ca: Option<Vec<u8>>,
    certificate: PathBuf,
    /// The file's modification time when the client in use was built.
    current: RwLock<(Option<SystemTime>, reqwest::Client)>,
}

impl std::fmt::Debug for LakeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let following = matches!(*self.0, Inner::Following(_));
        f.debug_struct("LakeClient")
            .field("following", &following)
            .finish()
    }
}

impl From<reqwest::Client> for LakeClient {
    fn from(http: reqwest::Client) -> Self {
        Self(Arc::new(Inner::Fixed(http)))
    }
}

impl LakeClient {
    /// From `LAKE_CA_FILE` and `LAKE_CLIENT_CERT_FILE`, each optional.
    pub fn from_env(timeout: Duration) -> Result<Self, String> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let ca = match var("LAKE_CA_FILE") {
            Some(path) => {
                Some(std::fs::read(&path).map_err(|e| format!("LAKE_CA_FILE {path}: {e}"))?)
            }
            None => None,
        };
        match var("LAKE_CLIENT_CERT_FILE") {
            Some(path) => Self::following(timeout, ca, Path::new(&path)),
            None => build(timeout, ca.as_deref(), None).map(Self::from),
        }
    }

    /// A client that presents the certificate and key of `certificate` (PEM,
    /// the certificate then the key) and follows that file.
    pub fn following(
        timeout: Duration,
        ca: Option<Vec<u8>>,
        certificate: &Path,
    ) -> Result<Self, String> {
        let (at, pem) = read(certificate)?;
        let http = build(timeout, ca.as_deref(), Some(&pem))?;
        Ok(Self(Arc::new(Inner::Following(Following {
            timeout,
            ca,
            certificate: certificate.to_path_buf(),
            current: RwLock::new((at, http)),
        }))))
    }

    /// The client to send a request with: the one in use, or one made with
    /// the certificate's file if it changed since.
    pub fn get(&self) -> reqwest::Client {
        match &*self.0 {
            Inner::Fixed(http) => http.clone(),
            Inner::Following(following) => following.get(),
        }
    }
}

impl Following {
    fn get(&self) -> reqwest::Client {
        let now = modified(&self.certificate);
        {
            let current = self.current.read().unwrap_or_else(|e| e.into_inner());
            if now.is_none() || now == current.0 {
                return current.1.clone();
            }
        }
        let made = read(&self.certificate)
            .and_then(|(at, pem)| Ok((at, build(self.timeout, self.ca.as_deref(), Some(&pem))?)));
        let mut current = self.current.write().unwrap_or_else(|e| e.into_inner());
        match made {
            Ok(made) => *current = made,
            Err(e) => tracing::error!("the lake's client certificate was not read again: {e}"),
        }
        current.1.clone()
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The file and when it was written. The error names the variable, not the
/// content.
fn read(path: &Path) -> Result<(Option<SystemTime>, Vec<u8>), String> {
    let at = modified(path);
    let pem = std::fs::read(path).map_err(|e| format!("LAKE_CLIENT_CERT_FILE: {e}"))?;
    Ok((at, pem))
}

fn build(
    timeout: Duration,
    ca: Option<&[u8]>,
    identity: Option<&[u8]>,
) -> Result<reqwest::Client, String> {
    let mut http = reqwest::Client::builder().timeout(timeout);
    if let Some(pem) = ca {
        let cert = reqwest::Certificate::from_pem(pem).map_err(|e| format!("LAKE_CA_FILE: {e}"))?;
        http = http.add_root_certificate(cert);
    }
    if let Some(pem) = identity {
        let identity =
            reqwest::Identity::from_pem(pem).map_err(|e| format!("LAKE_CLIENT_CERT_FILE: {e}"))?;
        http = http.identity(identity);
    }
    http.build().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};

    use super::*;

    /// A certificate then its key, as the agent's `machine.pem`.
    fn machine_pem() -> String {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let params = CertificateParams::new(vec!["app-1".to_string()]).unwrap();
        let cert = params.self_signed(&key).unwrap();
        format!("{}{}", cert.pem(), key.serialize_pem())
    }

    fn written_at(following: &LakeClient) -> Option<SystemTime> {
        match &*following.0 {
            Inner::Following(f) => f.current.read().unwrap().0,
            Inner::Fixed(_) => None,
        }
    }

    /// A directory of this test's own, removed when dropped.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("qd-lake-http-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn set_modified(path: &Path, at: SystemTime) {
        let file = File::options().write(true).open(path).unwrap();
        file.set_modified(at).unwrap();
    }

    #[test]
    fn the_certificate_is_read_again_only_when_its_file_changes() {
        let dir = Dir::new("renewed");
        let path = dir.path().join("machine.pem");
        std::fs::write(&path, machine_pem()).unwrap();
        let first = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        set_modified(&path, first);
        let client = LakeClient::following(Duration::from_secs(5), None, &path).unwrap();
        assert_eq!(written_at(&client), Some(first));
        client.get();
        assert_eq!(written_at(&client), Some(first), "unchanged: not rebuilt");

        // Renewed: written aside and renamed, as the agent does.
        let aside = dir.path().join(".machine.pem.new");
        std::fs::write(&aside, machine_pem()).unwrap();
        let second = first + Duration::from_secs(60);
        set_modified(&aside, second);
        std::fs::rename(&aside, &path).unwrap();
        client.get();
        assert_eq!(
            written_at(&client),
            Some(second),
            "the renewed one is in use"
        );

        // Not a certificate: the client in use stays, and it is not tried
        // again until the file changes once more.
        std::fs::write(&path, "not a certificate").unwrap();
        set_modified(&path, second + Duration::from_secs(60));
        client.get();
        assert_eq!(written_at(&client), Some(second));
        // Gone for a moment: the client in use stays.
        std::fs::remove_file(&path).unwrap();
        client.get();
        assert_eq!(written_at(&client), Some(second));
    }

    #[test]
    fn a_file_that_is_no_certificate_and_key_makes_no_client() {
        let dir = Dir::new("bad");
        let path = dir.path().join("machine.pem");
        let missing = LakeClient::following(Duration::from_secs(5), None, &path);
        assert!(missing.unwrap_err().starts_with("LAKE_CLIENT_CERT_FILE"));
        std::fs::write(&path, "-----BEGIN NOTHING-----").unwrap();
        let bad = LakeClient::following(Duration::from_secs(5), None, &path);
        let error = bad.unwrap_err();
        assert!(error.starts_with("LAKE_CLIENT_CERT_FILE"), "{error}");
        assert!(!error.contains("NOTHING"), "the file's content is not said");
        // One the caller made is used as it is.
        let fixed = LakeClient::from(reqwest::Client::new());
        assert_eq!(written_at(&fixed), None);
        fixed.get();
    }
}
