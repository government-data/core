//! A lake node's drives given in Settings (0066 step 11, the user's ruling
//! of 2026-11-13): the person may give the mounted paths of the replica's
//! drives when asking for its join token, and the command then carries a
//! `--drive` for each; or give none, and add them to the command by hand.
//! Both stay possible. The paths go into a shell command, so each is
//! checked here as `join.sh` checks it, before any token is issued: an
//! absolute path of letters, digits, `.`, `_`, `-` and `/`, with no `.` or
//! `..` segment. What is on the server (a filesystem of its own, mounted,
//! empty) only the script can check.

use auth_gateway::MachineRole;

/// At most this many drives on one server: a bound against a mistaken or
/// abusive request, not a limit of RustFS's that was measured (one to four
/// were tried, 0066 step 11).
pub const MAX_DRIVES: usize = 16;

/// The longest path a drive may have.
const MAX_DRIVE_PATH: usize = 255;

/// The drives a command carries, checked: none, or each a safe absolute
/// path, once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Drives(Vec<String>);

/// Why the drives given were refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrivesError {
    /// The role installs no drives.
    NotTaken,
    /// A path is not absolute, has a character out of the set, a `.` or
    /// `..` segment, or is too long.
    Invalid,
    /// More than [`MAX_DRIVES`].
    TooMany,
    /// The same path twice.
    Repeated,
}

/// Whether the installation of `role` takes drives: the lake's replica and
/// a pool's node (0066 step 11, the lake roles' ruling 5).
fn takes_drives(role: MachineRole) -> bool {
    matches!(role, MachineRole::LakeReplica | MachineRole::Lake)
}

impl Drives {
    /// The drives `paths` give for `role`; blank entries are ignored and one
    /// trailing `/` is dropped, as the script does.
    pub fn parse(role: MachineRole, paths: &[String]) -> Result<Self, DrivesError> {
        let given: Vec<&str> = paths
            .iter()
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .collect();
        if given.is_empty() {
            return Ok(Self::default());
        }
        if !takes_drives(role) {
            return Err(DrivesError::NotTaken);
        }
        if given.len() > MAX_DRIVES {
            return Err(DrivesError::TooMany);
        }
        let mut drives: Vec<String> = Vec::with_capacity(given.len());
        for path in given {
            let path = path.strip_suffix('/').unwrap_or(path);
            if !is_drive_path(path) {
                return Err(DrivesError::Invalid);
            }
            if drives.iter().any(|d| d == path) {
                return Err(DrivesError::Repeated);
            }
            drives.push(path.to_string());
        }
        Ok(Self(drives))
    }

    /// The paths, in the order given.
    pub fn paths(&self) -> &[String] {
        &self.0
    }

    /// ` --drive '<path>'` for each, as the command carries them.
    pub(super) fn arguments(&self) -> String {
        self.0.iter().map(|d| format!(" --drive '{d}'")).collect()
    }
}

/// An absolute path of the characters `join.sh` takes, without `.` or `..`
/// segments, up to [`MAX_DRIVE_PATH`].
fn is_drive_path(path: &str) -> bool {
    let safe = |c: char| c.is_ascii_alphanumeric() || "._-/".contains(c);
    let Some(rest) = path.strip_prefix('/') else {
        return false;
    };
    !rest.is_empty()
        && path.len() <= MAX_DRIVE_PATH
        && path.chars().all(safe)
        && !rest.split('/').any(|s| s == "." || s == "..")
}

#[cfg(test)]
mod tests {
    use super::*;
    use MachineRole::{Lake, LakeReplica, TrinoWorker, Worker};

    fn parse(role: MachineRole, paths: &[&str]) -> Result<Drives, DrivesError> {
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        Drives::parse(role, &paths)
    }

    #[test]
    fn the_replica_takes_safe_absolute_paths_in_order() {
        let drives = parse(LakeReplica, &["/srv/d1", " /mnt/disk_2/ ", "", "/a.b-c"]).unwrap();
        assert_eq!(drives.paths(), ["/srv/d1", "/mnt/disk_2", "/a.b-c"]);
        assert_eq!(
            drives.arguments(),
            " --drive '/srv/d1' --drive '/mnt/disk_2' --drive '/a.b-c'"
        );
    }

    #[test]
    fn none_given_is_none_for_any_role_and_others_take_none() {
        for role in [Worker, Lake, LakeReplica] {
            assert_eq!(parse(role, &["  "]), Ok(Drives::default()));
        }
        assert_eq!(parse(Worker, &["/srv/d1"]), Err(DrivesError::NotTaken));
        assert_eq!(parse(TrinoWorker, &["/srv/d1"]), Err(DrivesError::NotTaken));
        let node = parse(Lake, &["/srv/d1"]).unwrap();
        assert_eq!(node.paths(), ["/srv/d1"], "a pool's node takes them");
    }

    #[test]
    fn paths_that_could_break_the_command_are_refused() {
        let long = format!("/{}", "a".repeat(MAX_DRIVE_PATH));
        for bad in [
            "srv/d1",
            "/",
            "//",
            "/srv/d 1",
            "/srv/d1'; rm -rf /",
            "/srv/$(id)",
            "/srv/`id`",
            "/srv/d1\n--token x",
            "/srv/\"d1\"",
            "/srv/../etc",
            "/srv/./d1",
            "/srv/dísco",
            long.as_str(),
        ] {
            assert_eq!(
                parse(LakeReplica, &[bad]),
                Err(DrivesError::Invalid),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn too_many_or_the_same_twice_are_refused() {
        let many: Vec<String> = (0..=MAX_DRIVES).map(|n| format!("/srv/d{n}")).collect();
        assert_eq!(Drives::parse(LakeReplica, &many), Err(DrivesError::TooMany));
        let most = &many[..MAX_DRIVES];
        assert_eq!(
            Drives::parse(LakeReplica, most).unwrap().paths().len(),
            MAX_DRIVES
        );
        let twice = parse(LakeReplica, &["/srv/d1", "/srv/d1/"]);
        assert_eq!(twice, Err(DrivesError::Repeated));
    }
}
