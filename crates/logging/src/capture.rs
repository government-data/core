//! The lines a piece of code writes, for a service's tests.

use std::io;
use std::sync::{Arc, Mutex, Once};

use serde_json::{Map, Value};
use tracing::subscriber::Interest;
use tracing::{span, Event, Metadata};

static ASK_THE_THREAD: Once = Once::new();

/// The global subscriber of a test binary: it records nothing and answers
/// "sometimes" for every callsite, so `tracing` asks the thread's own
/// subscriber at each event. Without it, a parallel test that reaches a
/// callsite first, with no subscriber, can cache "never" for it while
/// another test installs its capture, and that test loses the line by
/// chance.
struct AskTheThread;

impl tracing::Subscriber for AskTheThread {
    fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
        Interest::sometimes()
    }
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        false
    }
    fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }
    fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
    fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
    fn event(&self, _: &Event<'_>) {}
    fn enter(&self, _: &span::Id) {}
    fn exit(&self, _: &span::Id) {}
}

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
    ASK_THE_THREAD.call_once(|| {
        // A test binary never calls `crate::init`: this is its only global.
        let _ = tracing::subscriber::set_global_default(AskTheThread);
    });
    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        f()
    });
    let bytes = out.0.lock().map(|b| b.clone()).unwrap_or_default();
    String::from_utf8_lossy(&bytes)
        .lines()
        .map(|l| serde_json::from_str(l).expect("a JSON object per line"))
        .collect()
}
