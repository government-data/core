//! The files of each module a machine's role runs (quality-sdk decision
//! 0013, contract e), as the core hands them with the registry token: put
//! under `<root>/<module>/`, each checked against its SHA-256 first. A
//! module's files are written into `<module>.new` and renamed over the
//! folder before it only once every file checked, so a bad answer leaves
//! the files in use as they were.

use std::path::{Component, Path};

use base64::Engine as _;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// A module of the role, as the core names it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RoleModule {
    pub name: String,
    pub version: String,
    pub services: Vec<String>,
    #[serde(default)]
    pub files: Vec<RoleFile>,
}

/// One of its files.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct RoleFile {
    pub path: String,
    pub sha256: String,
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

/// Put every module's files under `root`; the first module whose files do
/// not check stops it, with its name and why.
pub fn place(root: &Path, modules: &[RoleModule]) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
    for m in modules {
        place_module(root, m).map_err(|e| format!("module {}: {e}", m.name))?;
    }
    Ok(())
}

fn place_module(root: &Path, m: &RoleModule) -> Result<(), String> {
    if !plain(&m.name) || m.name.contains('/') {
        return Err("not a module's name".into());
    }
    let next = root.join(format!("{}.new", m.name));
    let _ = std::fs::remove_dir_all(&next);
    let written = write_all(&next, &m.files);
    if let Err(e) = written {
        let _ = std::fs::remove_dir_all(&next);
        return Err(e);
    }
    let current = root.join(&m.name);
    let _ = std::fs::remove_dir_all(&current);
    std::fs::rename(&next, &current).map_err(|e| e.to_string())
}

fn write_all(dir: &Path, files: &[RoleFile]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for f in files {
        if !plain(&f.path) {
            return Err(format!("{}: a path outside the module", f.path));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&f.content_base64)
            .map_err(|_| format!("{}: not base64", f.path))?;
        let digest: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if digest != f.sha256 {
            return Err(format!("{}: its SHA-256 does not match", f.path));
        }
        let path = dir.join(&f.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", f.path))?;
    }
    Ok(())
}

/// Relative, with plain segments only.
fn plain(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 255
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn file(path: &str, bytes: &[u8]) -> RoleFile {
        RoleFile {
            path: path.into(),
            sha256: Sha256::digest(bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    fn module(name: &str, files: Vec<RoleFile>) -> RoleModule {
        RoleModule {
            name: name.into(),
            version: "0.2.1".into(),
            services: vec!["rustfs".into()],
            files,
        }
    }

    #[test]
    fn each_modules_files_go_under_its_folder() {
        let dir = TempDir::new();
        let m = module(
            "lake-rustfs",
            vec![
                file("compose.yml", b"services: {}\n"),
                file("compose/gate.sh", b"#!/bin/sh\n"),
            ],
        );
        place(dir.path(), &[m]).unwrap();
        let read =
            |p: &str| std::fs::read_to_string(dir.path().join("lake-rustfs").join(p)).unwrap();
        assert_eq!(read("compose.yml"), "services: {}\n");
        assert_eq!(read("compose/gate.sh"), "#!/bin/sh\n");
    }

    #[test]
    fn a_file_that_does_not_check_leaves_the_files_in_use() {
        let dir = TempDir::new();
        place(
            dir.path(),
            &[module("lake-rustfs", vec![file("compose.yml", b"v1")])],
        )
        .unwrap();
        let mut wrong = file("compose.yml", b"v2");
        wrong.sha256 = "0".repeat(64);
        for bad in [
            vec![wrong],
            vec![file("../escape.sh", b"x")],
            vec![file("/etc/passwd", b"x")],
        ] {
            assert!(place(dir.path(), &[module("lake-rustfs", bad)]).is_err());
        }
        let kept = std::fs::read_to_string(dir.path().join("lake-rustfs/compose.yml")).unwrap();
        assert_eq!(kept, "v1");
        assert!(!dir.path().join("lake-rustfs.new").exists());
        assert!(!dir.path().join("escape.sh").exists());
        assert!(place(dir.path(), &[module("../x", vec![])]).is_err());
    }
}
