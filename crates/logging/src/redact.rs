//! The redaction rule (decision 0026, point 5) and the pretty format's field
//! formatter that applies it.

use std::fmt;

use tracing::field::{Field, Visit};
use tracing_subscriber::field::{MakeVisitor, VisitFmt, VisitOutput};
use tracing_subscriber::fmt::format::{DefaultFields, Writer};

/// What a sensitive field's value is replaced by, in both formats.
pub const REDACTED: &str = "[redacted]";

/// The names that mark a field as sensitive. See [`is_sensitive`].
const SENSITIVE: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "authorization",
    "cookie",
    "api_key",
    "private_key",
    // sqlx's statement lines carry the SQL text here (`summary` keeps only
    // its first four words).
    "db.statement",
    // Personal data (point 5); the identity crates log a person's address.
    "email",
];

/// Whether a field's value must not be logged: its name, ignoring case, is one
/// of the sensitive names, or ends with one after a `_`, `.` or `-`
/// (`client_secret`, `access_token`, `http.authorization`, `set-cookie`).
/// Only whole words at the end count, so `job_id`, `tokens_used` or
/// `secret_count` are kept.
pub fn is_sensitive(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SENSITIVE.iter().any(|word| {
        name == *word
            || name
                .strip_suffix(word)
                .is_some_and(|rest| rest.ends_with(['_', '.', '-']))
    })
}

/// The pretty format's fields: `tracing-subscriber`'s own, with a sensitive
/// field's value replaced by [`REDACTED`]. Used for events and spans alike.
#[derive(Debug, Default)]
pub(crate) struct RedactedFields(DefaultFields);

impl<'a> MakeVisitor<Writer<'a>> for RedactedFields {
    type Visitor = Redacting<<DefaultFields as MakeVisitor<Writer<'a>>>::Visitor>;

    fn make_visitor(&self, target: Writer<'a>) -> Self::Visitor {
        Redacting(self.0.make_visitor(target))
    }
}

/// Passes every field on to the inner visitor, a sensitive one as the marker.
#[derive(Debug)]
pub(crate) struct Redacting<V>(V);

impl<V: Visit> Redacting<V> {
    fn hide(&mut self, field: &Field) -> bool {
        let hidden = is_sensitive(field.name());
        if hidden {
            self.0.record_str(field, REDACTED);
        }
        hidden
    }
}

impl<V: Visit> Visit for Redacting<V> {
    fn record_f64(&mut self, field: &Field, value: f64) {
        if !self.hide(field) {
            self.0.record_f64(field, value);
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if !self.hide(field) {
            self.0.record_i64(field, value);
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if !self.hide(field) {
            self.0.record_u64(field, value);
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if !self.hide(field) {
            self.0.record_bool(field, value);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if !self.hide(field) {
            self.0.record_str(field, value);
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        if !self.hide(field) {
            self.0.record_error(field, value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if !self.hide(field) {
            self.0.record_debug(field, value);
        }
    }
}

impl<V: VisitOutput<fmt::Result>> VisitOutput<fmt::Result> for Redacting<V> {
    fn finish(self) -> fmt::Result {
        self.0.finish()
    }
}

impl<V: VisitFmt> VisitFmt for Redacting<V> {
    fn writer(&mut self) -> &mut dyn fmt::Write {
        self.0.writer()
    }
}
