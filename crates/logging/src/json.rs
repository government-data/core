//! The JSON format: one flat object per line (decision 0026, point 1).

use std::collections::HashSet;
use std::fmt;
use std::io::Write as _;

use serde_json::Value;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

use crate::redact::{is_sensitive, REDACTED};

/// The keys every line starts with; a field of the same name is written as
/// `field.<name>` instead.
const RESERVED: &[&str] = &["time", "level", "service", "vm", "target", "msg"];

/// The ids lifted to the top level, from the event or the innermost span in
/// scope that has them.
const IDS: &[&str] = &["tenant", "request_id", "run_id", "job_id"];

/// What a `log` record bridged into `tracing` carries besides its message.
const LOG_FIELDS: &[&str] = &["log.target", "log.module_path", "log.file", "log.line"];

/// The fields of one event or one span, in the order they were recorded,
/// sensitive values already replaced.
#[derive(Debug, Default, Clone)]
pub(crate) struct Fields {
    pub(crate) message: Option<String>,
    pub(crate) values: Vec<(String, Value)>,
}

impl Fields {
    fn put(&mut self, field: &Field, value: Value) {
        let name = field.name();
        if name == "message" {
            self.message = Some(match value {
                Value::String(text) => text,
                other => other.to_string(),
            });
            return;
        }
        let value = if is_sensitive(name) {
            Value::from(REDACTED)
        } else {
            value
        };
        match self.values.iter_mut().find(|(key, _)| key == name) {
            Some(slot) => slot.1 = value,
            None => self.values.push((name.to_string(), value)),
        }
    }

    fn get(&self, name: &str) -> Option<&Value> {
        self.values
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, v)| v)
    }
}

impl Visit for Fields {
    fn record_f64(&mut self, field: &Field, value: f64) {
        let number = serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or_else(|| Value::from(value.to_string()));
        self.put(field, number);
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.put(field, Value::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.put(field, Value::from(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.put(field, Value::from(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.put(field, Value::from(value));
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.put(field, Value::from(value.to_string()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.put(field, Value::from(format!("{value:?}")));
    }
}

/// What a line is made of, before it is written.
#[derive(Debug)]
pub(crate) struct Line<'a> {
    pub(crate) time: &'a str,
    pub(crate) level: &'a str,
    pub(crate) service: &'a str,
    pub(crate) vm: &'a str,
    pub(crate) target: &'a str,
    pub(crate) event: &'a Fields,
    /// The spans in scope, innermost first.
    pub(crate) spans: &'a [Fields],
}

/// Writes keys in order, each once: the first value given for a key wins.
struct Object {
    buf: Vec<u8>,
    seen: HashSet<String>,
}

impl Object {
    fn entry(&mut self, key: &str, value: &Value) {
        if !self.seen.insert(key.to_string()) {
            return;
        }
        if self.seen.len() > 1 {
            self.buf.push(b',');
        }
        // Writing to a Vec cannot fail, and a `Value` always serializes.
        let _ = serde_json::to_writer(&mut self.buf, key);
        self.buf.push(b':');
        let _ = serde_json::to_writer(&mut self.buf, value);
    }

    fn field(&mut self, key: &str, value: &Value) {
        if RESERVED.contains(&key) {
            self.entry(&format!("field.{key}"), value);
        } else {
            self.entry(key, value);
        }
    }
}

/// Renders one line: the reserved keys, then the ids, then the event's
/// fields, then the spans' fields from the innermost out, ending in `\n`.
pub(crate) fn render(line: &Line<'_>) -> Vec<u8> {
    let mut object = Object {
        buf: vec![b'{'],
        seen: HashSet::new(),
    };
    let target = match line.event.get("log.target") {
        Some(Value::String(target)) => target.as_str(),
        _ => line.target,
    };
    object.entry("time", &Value::from(line.time));
    object.entry("level", &Value::from(line.level));
    object.entry("service", &Value::from(line.service));
    object.entry("vm", &Value::from(line.vm));
    object.entry("target", &Value::from(target));
    if let Some(message) = &line.event.message {
        object.entry("msg", &Value::from(message.as_str()));
    }
    for id in IDS {
        let found = line
            .event
            .get(id)
            .or_else(|| line.spans.iter().find_map(|span| span.get(id)));
        if let Some(value) = found {
            object.field(id, value);
        }
    }
    let event = line.event.values.iter();
    for (key, value) in event.filter(|(key, _)| !LOG_FIELDS.contains(&key.as_str())) {
        object.field(key, value);
    }
    for span in line.spans {
        for (key, value) in &span.values {
            object.field(key, value);
        }
    }
    object.buf.extend_from_slice(b"}\n");
    object.buf
}

/// The layer that writes each event as one JSON line to `writer`.
#[derive(Debug)]
pub(crate) struct JsonLayer<W> {
    pub(crate) service: String,
    pub(crate) vm: String,
    pub(crate) writer: W,
}

impl<S, W> Layer<S> for JsonLayer<W>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: for<'a> MakeWriter<'a> + 'static,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut fields = Fields::default();
        attrs.record(&mut fields);
        span.extensions_mut().insert(fields);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut extensions = span.extensions_mut();
        if let Some(fields) = extensions.get_mut::<Fields>() {
            values.record(fields);
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let spans: Vec<Fields> = ctx
            .event_scope(event)
            .map(|scope| {
                scope
                    .map(|span| {
                        span.extensions()
                            .get::<Fields>()
                            .cloned()
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let time = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        let metadata = event.metadata();
        let line = render(&Line {
            time: &time,
            level: metadata.level().as_str(),
            service: &self.service,
            vm: &self.vm,
            target: metadata.target(),
            event: &fields,
            spans: &spans,
        });
        // A log line that cannot be written has nowhere to be reported.
        let _ = self.writer.make_writer_for(metadata).write_all(&line);
    }
}
