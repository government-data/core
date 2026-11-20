//! The lines a piece of code writes, for a service's tests.

use std::io;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| io::ErrorKind::Other)?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The JSON lines `f` writes as [`crate::init`] would for `service` (with
/// `vm` `vm-test`), each parsed. `f` runs with this subscriber as the
/// thread's default: to log from a future, block on it inside `f`.
///
/// # Panics
///
/// If a line written is not a JSON object.
pub fn json_lines(service: &str, f: impl FnOnce()) -> Vec<Map<String, Value>> {
    let out = Captured::default();
    let writer = out.clone();
    let subscriber = crate::json_subscriber(service, "vm-test", move || writer.clone());
    tracing::subscriber::with_default(subscriber, || {
        // Another test may have reached a callsite first with no subscriber,
        // caching "never" for it; tests run in parallel, so ask again now that
        // this one is the default, or its lines are lost by chance.
        tracing::callsite::rebuild_interest_cache();
        f()
    });
    let bytes = out.0.lock().map(|b| b.clone()).unwrap_or_default();
    String::from_utf8_lossy(&bytes)
        .lines()
        .map(|l| serde_json::from_str(l).expect("a JSON object per line"))
        .collect()
}
