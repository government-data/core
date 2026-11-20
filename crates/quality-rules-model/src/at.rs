//! A value with the place it was written, so lint can point at it.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer};

/// Where a value starts in the YAML: line and column, from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub line: u64,
    pub column: u64,
}

/// A value read from YAML with its [`Position`]. In the JSON Schema it is
/// just the value.
#[derive(Debug, Clone, PartialEq)]
pub struct At<T> {
    pub value: T,
    pub at: Position,
}

impl<T> At<T> {
    /// A value with no place in a file (built in code, in tests).
    pub fn new(value: T) -> Self {
        Self {
            value,
            at: Position { line: 0, column: 0 },
        }
    }
}

/// An empty value with no place: what a named rule has for its `id`.
impl<T: Default> Default for At<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for At<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let spanned = serde_saphyr::Spanned::<T>::deserialize(deserializer)?;
        let at = Position {
            line: spanned.referenced.line(),
            column: spanned.referenced.column(),
        };
        Ok(Self {
            value: spanned.value,
            at,
        })
    }
}

impl<T: JsonSchema> JsonSchema for At<T> {
    fn inline_schema() -> bool {
        T::inline_schema()
    }

    fn schema_name() -> Cow<'static, str> {
        T::schema_name()
    }

    fn schema_id() -> Cow<'static, str> {
        T::schema_id()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        T::json_schema(generator)
    }
}
