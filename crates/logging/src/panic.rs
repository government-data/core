//! A panic as one line of the log (decision 0026, point 1): without it, a
//! panic's text goes to standard error only, several lines with no level,
//! which Loki cannot tell from any other text.

use std::panic::{self, PanicHookInfo};
use std::sync::Once;

static INSTALL: Once = Once::new();

/// Adds, once per process, a panic hook that logs the panic as an event at
/// level error through the global subscriber, so in either format and with
/// the ids of the spans in scope (a job's `job_id`, a request's
/// `request_id`). It then calls the hook it replaced, so the text on
/// standard error is still written.
///
/// The line has `msg` `panic`, `location` (`file:line`) and `thread`; not
/// the panic's message, which is free text that may carry a value (point 5),
/// and no backtrace.
pub(crate) fn install() {
    INSTALL.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            log(info);
            previous(info);
        }));
    });
}

fn log(info: &PanicHookInfo<'_>) {
    let location = info.location().map_or_else(
        || "unknown".to_string(),
        |l| format!("{}:{}", l.file(), l.line()),
    );
    let thread = std::thread::current();
    let thread = thread.name().unwrap_or("<unnamed>");
    tracing::error!(location = %location, thread, "panic");
}
