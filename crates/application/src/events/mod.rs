//! Module events (quality-sdk decision 0017): what a module publishes is
//! validated against the JSON Schema it declares for the topic and
//! version. The validator is a port, so the domain stays pure; the
//! adapter compiles each schema as Draft 2020-12 and never fetches a
//! remote `$ref`.

use std::sync::Arc;

use serde_json::Value;

/// Compiles the schemas modules declare.
pub trait EventSchemas: Send + Sync {
    /// `schema` compiled, or why it does not compile.
    fn compile(&self, schema: &Value) -> Result<Arc<dyn EventSchema>, String>;
}

/// A compiled schema.
pub trait EventSchema: Send + Sync {
    /// `Ok` when `data` validates; otherwise the schema paths it fails at
    /// (`/properties/run_id/type`), never the values.
    fn validate(&self, data: &Value) -> Result<(), Vec<String>>;
}
