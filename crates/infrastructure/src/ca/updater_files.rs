//! The update agent's files for the internal entry point (decision 0003):
//! what `server updater-certificate` writes, in the directory the agent
//! reads them from (`/etc/quality-data/internal/` on the host).
//!
//! | File              | Mode | Holds                                   |
//! | ----------------- | ---- | --------------------------------------- |
//! | `ca.pem`          | 0644 | The installation's CA                   |
//! | `certificate.pem` | 0644 | The agent's certificate, `OU=updater`   |
//! | `private-key.pem` | 0600 | Its key, PKCS#8                         |
//!
//! The directory is 0700. Each file is written aside and renamed; the key
//! and its certificate are checked together when read back ([`held`]), so a
//! crash between the two renames is signed again, never used.
//!
//! [`held`]: UpdaterFiles::held

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rcgen::{KeyPair, PublicKeyData};
use x509_parser::prelude::{FromDer, X509Certificate};

use super::certificate_info;
use super::inspect::pem_contents;

pub const CA_FILE: &str = "ca.pem";
pub const CERTIFICATE_FILE: &str = "certificate.pem";
pub const PRIVATE_KEY_FILE: &str = "private-key.pem";

/// The certificate's role, its `OU`.
const ROLE: &str = "updater";

/// A certificate with less than this left is signed again.
pub const RENEW_BEFORE: Duration = Duration::from_secs(30 * 24 * 3600);

/// The agent's three files, in one directory.
#[derive(Debug, Clone)]
pub struct UpdaterFiles {
    dir: PathBuf,
}

impl UpdaterFiles {
    /// The files in `dir`, made 0700 if it is not there.
    pub fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            dir: dir.to_path_buf(),
        })
    }

    pub fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    /// The serial of the certificate there, when it is the update agent's
    /// `name`, of the key beside it, and lasts more than [`RENEW_BEFORE`]
    /// after `now`. Otherwise `None`: it is to be signed (again).
    pub fn held(&self, name: &str, now: SystemTime) -> Option<String> {
        let cert_pem = fs::read_to_string(self.path(CERTIFICATE_FILE)).ok()?;
        let key_pem = fs::read_to_string(self.path(PRIVATE_KEY_FILE)).ok()?;
        if !of_key(&cert_pem, &key_pem) {
            return None;
        }
        let info = certificate_info(&cert_pem).ok()?;
        let not_after =
            UNIX_EPOCH + Duration::from_secs(info.not_after.timestamp().try_into().ok()?);
        let fresh = not_after > now + RENEW_BEFORE;
        let ours = info.unit.as_deref() == Some(ROLE) && info.common_name.as_deref() == Some(name);
        (fresh && ours).then_some(info.serial)
    }

    /// Writes the CA, the key and the certificate, each aside then renamed.
    pub fn write(&self, ca_pem: &str, cert_pem: &str, key_pem: &str) -> io::Result<()> {
        let files = [
            (CA_FILE, ca_pem, 0o644),
            (PRIVATE_KEY_FILE, key_pem, 0o600),
            (CERTIFICATE_FILE, cert_pem, 0o644),
        ];
        for (name, contents, mode) in files {
            write_synced(&self.aside(name), contents, mode)?;
        }
        for (name, _, _) in files {
            fs::rename(self.aside(name), self.path(name))?;
        }
        File::open(&self.dir)?.sync_all()
    }

    fn aside(&self, name: &str) -> PathBuf {
        self.dir.join(format!(".{name}.new"))
    }
}

/// Whether the certificate `cert_pem` is of the key `key_pem`.
fn of_key(cert_pem: &str, key_pem: &str) -> bool {
    let Ok(key) = KeyPair::from_pem(key_pem) else {
        return false;
    };
    let Ok(der) = pem_contents(cert_pem, "CERTIFICATE") else {
        return false;
    };
    let Ok((_, cert)) = X509Certificate::from_der(&der) else {
        return false;
    };
    cert.public_key().subject_public_key.data.as_ref() == key.der_bytes()
}

fn write_synced(path: &Path, contents: &str, mode: u32) -> io::Result<()> {
    let _ = fs::remove_file(path);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)?;
    // The exact mode, whatever the umask.
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ca::{Ca, Lifetime, MachineIdentity, DEFAULT_CA_VALIDITY};

    fn signed(ca: &Ca, role: &str, name: &str) -> (String, String) {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let csr = rcgen::CertificateParams::new(vec![name.into()])
            .unwrap()
            .serialize_request(&key)
            .unwrap()
            .pem()
            .unwrap();
        let identity = MachineIdentity::new(role, name, [name]).unwrap();
        let issued = ca.sign(&csr, &identity, Lifetime::CEILING).unwrap();
        (issued.pem, key.serialize_pem())
    }

    fn dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qd-updater-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn written_files_are_held_while_they_last_and_are_the_agents() {
        let made = Ca::create("test", DEFAULT_CA_VALIDITY).unwrap();
        let ca = Ca::load(&made.cert_pem, &made.key_pem).unwrap();
        let dir = dir("held");
        let files = UpdaterFiles::open(&dir).unwrap();
        let now = SystemTime::now();
        assert_eq!(files.held("updater", now), None, "nothing there");

        let (cert, key) = signed(&ca, "updater", "updater");
        files.write(&made.cert_pem, &cert, &key).unwrap();
        let serial = certificate_info(&cert).unwrap().serial;
        assert_eq!(files.held("updater", now), Some(serial));
        assert_eq!(files.held("other", now), None, "another name");
        let late = now + Duration::from_secs(340 * 24 * 3600);
        assert_eq!(files.held("updater", late), None, "less than 30 days left");

        let mode = |f: &str| fs::metadata(files.path(f)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(PRIVATE_KEY_FILE), 0o600);
        assert_eq!(mode(CERTIFICATE_FILE), 0o644);
        assert_eq!(
            fs::read_to_string(files.path(CA_FILE)).unwrap(),
            made.cert_pem
        );
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );

        // A key that is not the certificate's, or a machine of another role.
        let (_, other_key) = signed(&ca, "updater", "updater");
        fs::write(files.path(PRIVATE_KEY_FILE), other_key).unwrap();
        assert_eq!(files.held("updater", now), None, "not of this key");
        let (app, app_key) = signed(&ca, "app", "updater");
        files.write(&made.cert_pem, &app, &app_key).unwrap();
        assert_eq!(files.held("updater", now), None, "role app");
        fs::remove_dir_all(&dir).unwrap();
    }
}
