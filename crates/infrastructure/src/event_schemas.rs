//! The modules' event schemas compiled with `jsonschema` (quality-sdk
//! decision 0017, section 5): always as Draft 2020-12, with a retriever
//! that refuses every URI, so a `$ref` outside the schema never compiles
//! even if another crate turns the crate's resolvers on (verified by
//! `tools/verify/jsonschema-draft`). An error names the schema path only:
//! the crate's own message quotes the value, which may be anything.

use std::sync::Arc;

use application::events::{EventSchema, EventSchemas};
use jsonschema::{Draft, Retrieve, Uri, Validator};
use serde_json::Value;

/// The adapter of [`EventSchemas`].
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonSchemas;

/// Refuses every `$ref` the schema does not hold itself.
struct NoRemote;

impl Retrieve for NoRemote {
    fn retrieve(
        &self,
        _uri: &Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("a $ref outside the schema is not followed".into())
    }
}

struct Compiled(Validator);

impl EventSchemas for JsonSchemas {
    fn compile(&self, schema: &Value) -> Result<Arc<dyn EventSchema>, String> {
        jsonschema::options()
            .with_draft(Draft::Draft202012)
            .with_retriever(NoRemote)
            .build(schema)
            .map(|v| Arc::new(Compiled(v)) as Arc<dyn EventSchema>)
            .map_err(|e| e.to_string())
    }
}

impl EventSchema for Compiled {
    fn validate(&self, data: &Value) -> Result<(), Vec<String>> {
        let failed: Vec<String> = self
            .0
            .iter_errors(data)
            .map(|e| e.schema_path().to_string())
            .collect();
        if failed.is_empty() {
            Ok(())
        } else {
            Err(failed)
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const DRAFT: &str = "https://json-schema.org/draft/2020-12/schema";

    #[test]
    fn a_2020_12_schema_compiles_and_validates_naming_paths_not_values() {
        let schema = json!({
            "$schema": DRAFT, "type": "object", "required": ["run_id"],
            "properties": {"run_id": {"type": "string"}},
            "unevaluatedProperties": false
        });
        let compiled = JsonSchemas.compile(&schema).unwrap();
        assert!(compiled.validate(&json!({"run_id": "r1"})).is_ok());
        let paths = compiled
            .validate(&json!({"run_id": "secret-value", "x": 1}))
            .unwrap_err();
        assert!(!paths.is_empty());
        assert!(
            paths.iter().all(|p| !p.contains("secret-value")),
            "{paths:?}"
        );
    }

    #[test]
    fn a_remote_or_file_ref_does_not_compile() {
        for uri in [
            "https://example.com/s.json",
            "http://127.0.0.1:9/s.json",
            "file:///etc/passwd",
        ] {
            let schema = json!({"$schema": DRAFT, "properties": {"a": {"$ref": uri}}});
            assert!(JsonSchemas.compile(&schema).is_err(), "{uri}");
        }
    }

    #[test]
    fn a_bad_keyword_does_not_compile() {
        assert!(JsonSchemas.compile(&json!({"type": "nonsense"})).is_err());
    }
}
