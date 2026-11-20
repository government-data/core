//! Technical logs as decision 0026 wants them, set up the same way by every
//! service: [`init`] with the service's name, once, at the start of a
//! long-running command.
//!
//! The filter comes from `RUST_LOG` (default `info`). The format from
//! `LOG_FORMAT`:
//!
//! - **JSON** (the default): one flat object per line. It starts with `time`
//!   (RFC 3339, UTC), `level` (`INFO`, …), `service`, `vm` (from `OBS_VM`, as
//!   Alloy reads it, else `dev`), `target` and `msg`; then `tenant`,
//!   `request_id`, `run_id` and `job_id`, each lifted from the event or from
//!   the innermost span in scope that has it, and absent when none has; then
//!   the event's other fields, then the spans' fields from the innermost out.
//!   Numbers and booleans stay numbers and booleans; anything formatted with
//!   `%` or `?` is a string.
//! - **`pretty`**: `tracing-subscriber`'s human text, for development.
//!
//! **Names that collide.** Each key is written once and the first value wins:
//! the event's field over a span's, an inner span's over an outer one's. A
//! field named like a reserved key (`time`, `level`, `service`, `vm`,
//! `target`, `msg`) is written as `field.<name>`, so it is kept without
//! replacing the line's own.
//!
//! **Redaction** (point 5). The value of a field whose name is sensitive
//! (see [`is_sensitive`]) is replaced by [`REDACTED`], in both formats, in
//! events and in spans. It works on field names only: it cannot see a value
//! written into a message (`info!("signed in {email}")`) or into an error's
//! text, nor personal data in a field with a harmless name. It is the second
//! line; the first is not logging such values at all.
//!
//! **Panics.** [`init`] also logs a panic as one line at level `ERROR`, with
//! `msg` `panic`, its `location` (`file:line`) and `thread`, in the format
//! chosen and with the ids of the spans in scope; the panic's own message,
//! free text that may carry a value, is left out of it. The text on
//! standard error is still written as before.
//!
//! **Metrics.** [`metrics`] writes the service's metrics file (decision
//! 0071, point 9), which the machine's Alloy reads beside these lines.

mod capture;
mod json;
pub mod metrics;
mod panic;
mod redact;

#[cfg(test)]
mod tests;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

pub use capture::json_lines;
pub use redact::{is_sensitive, REDACTED};

/// Installs the global subscriber for `service` (`server`, `account`, …), as
/// the crate's documentation says. Call it once per process.
pub fn init(service: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let pretty = std::env::var("LOG_FORMAT").is_ok_and(|v| v.eq_ignore_ascii_case("pretty"));
    let registry = tracing_subscriber::registry().with(filter);
    if pretty {
        registry.with(pretty_layer(std::io::stdout)).init();
    } else {
        registry
            .with(json_layer(service, &vm(), std::io::stdout))
            .init();
    }
    panic::install();
}

/// A subscriber writing [`init`]'s JSON lines to `writer`, unfiltered, with
/// `vm` given: for a service's tests that read the lines it writes.
pub fn json_subscriber<W>(service: &str, vm: &str, writer: W) -> impl tracing::Subscriber
where
    W: for<'a> fmt::MakeWriter<'a> + Send + Sync + 'static,
{
    tracing_subscriber::registry().with(json_layer(service, vm, writer))
}

/// The machine the process runs on, as Alloy labels it.
fn vm() -> String {
    std::env::var("OBS_VM")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "dev".to_string())
}

fn json_layer<W>(service: &str, vm: &str, writer: W) -> json::JsonLayer<W> {
    json::JsonLayer {
        service: service.to_string(),
        vm: vm.to_string(),
        writer,
    }
}

fn pretty_layer<S, W>(writer: W) -> impl tracing_subscriber::Layer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    W: for<'a> fmt::MakeWriter<'a> + 'static,
{
    fmt::layer()
        .with_target(true)
        .with_thread_ids(false)
        .fmt_fields(redact::RedactedFields::default())
        .with_writer(writer)
}
