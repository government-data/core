//! The agent's directory (0066, point 13): the machine's key and
//! certificate and the CA, written aside and renamed, never seen half done.
//!
//! | File          | Mode | Holds                                                 |
//! | ------------- | ---- | ----------------------------------------------------- |
//! | `ca.crt`      | 0644 | The installation's CA                                 |
//! | `machine.pem` | 0600 | The certificate then the key: what the agent presents |
//! | `machine.key` | 0600 | The key alone, PKCS#8                                 |
//! | `machine.crt` | 0644 | The certificate alone                                 |
//! | `next.pem`    | 0600 | A renewed certificate and its key, not yet in use     |
//! | `refused`     | 0644 | Written when the API refused the machine              |
//!
//! The directory is 0700. A renewal is first `next.pem`, one file, so it is
//! there whole or not at all. Promoting it writes the three `machine.*`
//! aside, then renames `machine.pem` (the agent's own, one file: never
//! mismatched), `machine.key`, `machine.crt`, and only then removes
//! `next.pem`: a crash at any point leaves `next.pem`, and promoting it
//! again finishes the work. Between the two renames a reader of the
//! separate files could see a new key with the old certificate, which is
//! why the hook runs only after both, and why the agent reads `machine.pem`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::cert::Credentials;

pub const CA: &str = "ca.crt";
pub const MACHINE_BUNDLE: &str = "machine.pem";
pub const MACHINE_KEY: &str = "machine.key";
pub const MACHINE_CERT: &str = "machine.crt";
pub const NEXT: &str = "next.pem";
pub const REFUSED: &str = "refused";

const SECRET: u32 = 0o600;
const PUBLIC: u32 = 0o644;

/// The agent's files, in one directory.
#[derive(Debug, Clone)]
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    /// The store in `dir`, made 0700 if it is not there.
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

    /// The credentials in use, if the machine joined.
    pub fn current(&self) -> io::Result<Option<Credentials>> {
        self.bundle(MACHINE_BUNDLE)
    }

    /// A renewed certificate waiting to be promoted.
    pub fn next(&self) -> io::Result<Option<Credentials>> {
        self.bundle(NEXT)
    }

    pub fn ca(&self) -> io::Result<Option<String>> {
        read_optional(&self.path(CA))
    }

    pub fn write_ca(&self, pem: &str) -> io::Result<()> {
        self.replace(CA, pem, PUBLIC)
    }

    /// Keeps `next` as the renewal waiting, replacing any before it.
    pub fn store_next(&self, next: &Credentials) -> io::Result<()> {
        self.replace(NEXT, &next.bundle(), SECRET)
    }

    /// Makes the renewal waiting the one in use, as the module says.
    /// `false` when there was none.
    pub fn promote(&self) -> io::Result<bool> {
        let Some(next) = self.next()? else {
            return Ok(false);
        };
        let files = [
            (MACHINE_BUNDLE, next.bundle(), SECRET),
            (MACHINE_KEY, next.key_pem.clone(), SECRET),
            (MACHINE_CERT, next.cert_pem.clone(), PUBLIC),
        ];
        for (name, contents, mode) in &files {
            write_synced(&self.aside(name), contents, *mode)?;
        }
        for (name, _, _) in &files {
            fs::rename(self.aside(name), self.path(name))?;
        }
        self.sync_dir()?;
        fs::remove_file(self.path(NEXT))?;
        self.sync_dir()?;
        Ok(true)
    }

    /// Marks the machine refused, so that a restart does not ask again.
    pub fn mark_refused(&self) -> io::Result<()> {
        self.replace(REFUSED, "refused\n", PUBLIC)
    }

    pub fn refused(&self) -> bool {
        self.path(REFUSED).exists()
    }

    fn bundle(&self, name: &str) -> io::Result<Option<Credentials>> {
        let Some(pem) = read_optional(&self.path(name))? else {
            return Ok(None);
        };
        let invalid =
            |e: String| io::Error::new(io::ErrorKind::InvalidData, format!("{name}: {e}"));
        Credentials::from_bundle(&pem).map(Some).map_err(invalid)
    }

    /// Writes `name` aside, then renames it over the old one.
    fn replace(&self, name: &str, contents: &str, mode: u32) -> io::Result<()> {
        write_synced(&self.aside(name), contents, mode)?;
        fs::rename(self.aside(name), self.path(name))?;
        self.sync_dir()
    }

    fn aside(&self, name: &str) -> PathBuf {
        self.dir.join(format!(".{name}.new"))
    }

    fn sync_dir(&self) -> io::Result<()> {
        File::open(&self.dir)?.sync_all()
    }
}

fn read_optional(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// A new file of `mode` (set when it is made, so it is never more open),
/// on disk before it returns.
fn write_synced(path: &Path, contents: &str, mode: u32) -> io::Result<()> {
    let _ = fs::remove_file(path);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()
}

#[cfg(test)]
mod tests;
