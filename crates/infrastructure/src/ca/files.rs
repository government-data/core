//! Writing a new CA's files, once: `server ca-init`.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use super::{CaError, NewCa};

/// Write the CA's certificate (mode 0644) and key (0600) to new files. If
/// either path exists nothing is written: a CA is never overwritten.
pub fn write_new(ca: &NewCa, cert_path: &Path, key_path: &Path) -> Result<(), CaError> {
    for path in [cert_path, key_path] {
        if path.exists() {
            return Err(CaError::File(format!(
                "{} exists; a CA is not overwritten",
                path.display()
            )));
        }
    }
    write_file(key_path, &ca.key_pem, 0o600)?;
    if let Err(e) = write_file(cert_path, &ca.cert_pem, 0o644) {
        // Without its certificate the key is of no use; leave nothing half made.
        let _ = std::fs::remove_file(key_path);
        return Err(e);
    }
    Ok(())
}

fn write_file(path: &Path, contents: &str, mode: u32) -> Result<(), CaError> {
    let error = |e: std::io::Error| CaError::File(format!("{}: {e}", path.display()));
    let mut file = create_new(path, mode).map_err(error)?;
    file.write_all(contents.as_bytes()).map_err(error)?;
    file.sync_all().map_err(error)
}

#[cfg(unix)]
fn create_new(path: &Path, mode: u32) -> std::io::Result<File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)?;
    // The exact mode, whatever the umask.
    file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    Ok(file)
}

#[cfg(not(unix))]
fn create_new(path: &Path, _mode: u32) -> std::io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}
