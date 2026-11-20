//! The node agent (decisions 0029, 0030, 0066): the machine's key and
//! certificate, gotten with its join token and renewed with the certificate
//! it has, with no person involved. The logic is here, behind ports; the
//! binary `qd-node` (`main.rs`) only reads its settings and wires them.
//!
//! - [`cert`]: the key, the signing request, a certificate's dates and the
//!   CA's fingerprint;
//! - [`store`]: the files, written aside and renamed;
//! - [`plan`]: what to do next, pure;
//! - [`api`]: the join and renew calls as a port, and what each answer
//!   means; [`http`] makes them over HTTPS;
//! - [`config`]: the settings;
//! - [`ports`]: the clock and the commands (the hook, held mode's check);
//! - [`agent`]: the loop that does what [`plan`] decides;
//! - [`trino`]: held mode's two commands for Trino's coordinator, whether
//!   Trino is free and its restart (0066, point 9);
//! - [`dolt`]: on the application VM, the follower that restarts Dolt when
//!   it is free to give it the certificate in use (point 11);
//! - [`restart`]: what both restarts share: the command, the wait and the
//!   release;
//! - [`metrics`]: its metrics file, renewals by result and the certificate's
//!   end (decision 0071).

/// The `service` of its log lines (decision 0026): `logging::init`'s name.
pub const SERVICE: &str = "node-agent";

pub mod agent;
pub mod api;
pub mod cert;
pub mod config;
pub mod dolt;
pub mod http;
pub mod metrics;
pub mod plan;
pub mod ports;
pub mod restart;
pub mod store;
pub mod trino;

#[cfg(test)]
pub(crate) mod testing {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A directory of its own for a test, removed when it ends.
    pub(crate) struct TempDir(PathBuf);

    impl TempDir {
        pub(crate) fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("node-agent-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        pub(crate) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
