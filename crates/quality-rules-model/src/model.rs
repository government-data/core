//! The rules' YAML as written (decisions 0019, 0033): what a dataset
//! declares once, its combination tables and its rules. Nothing here is
//! checked beyond its shape; `resolve` and `lint` do the rest.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::at::At;

pub use crate::rule::{
    Case, Cause, Dimension, Examples, NamedFormat, OnEmpty, Origin, Pattern, Records, Rule,
    Severity, ValueSet,
};

/// One rules file: a dataset's declarations and rules.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RulesFile {
    /// The dataset the rules read, as named in its mapping.
    pub dataset: At<String>,
    /// Named conditions a rule can apply to (`population: individuals`).
    #[serde(default)]
    pub populations: BTreeMap<String, At<String>>,
    /// Named lists of values, for `domain` (`in: { list: name }`) and
    /// `in_list(x, 'name')`.
    #[serde(default)]
    pub lists: BTreeMap<String, At<Vec<Text>>>,
    /// Fillers that mean "no value", declared once per type.
    #[serde(default)]
    pub placeholders: Placeholders,
    /// Fields computed against the cut, never against today.
    #[serde(default)]
    pub derived: BTreeMap<String, At<String>>,
    /// Other datasets a rule may look up, and which of their cuts.
    #[serde(default)]
    pub references: BTreeMap<String, Reference>,
    /// Tables of allowed or forbidden combinations, for `combination`.
    #[serde(default)]
    pub tables: BTreeMap<String, CombinationTable>,
    /// Rules with everything but their `id` and `field`, which a field's
    /// rule `extends` (decision 0045): only in the table's global rules.
    #[serde(default)]
    pub named: BTreeMap<String, At<Rule>>,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

/// A value as written: kept as text (codes keep their leading zeros) and
/// typed later by the field it is compared with. Numbers and booleans are
/// accepted in the YAML.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(transparent)]
pub struct Text(pub String);

/// A value: text, a number or a boolean, read as text.
#[derive(JsonSchema)]
#[serde(untagged)]
#[allow(dead_code)]
enum Scalar {
    Text(String),
    Number(f64),
    Boolean(bool),
}

impl JsonSchema for Text {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Value".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        Scalar::json_schema(generator)
    }
}

/// One field or several (`field: [A, B]` gives one rule per field).
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(untagged)]
pub enum Fields {
    One(String),
    Many(Vec<String>),
}

impl Fields {
    pub fn names(&self) -> Vec<&str> {
        match self {
            Self::One(name) => vec![name.as_str()],
            Self::Many(names) => names.iter().map(String::as_str).collect(),
        }
    }
}

/// The dataset's fillers by type: `required` counts them as empty and
/// `placeholder` finds them.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Placeholders {
    #[serde(default)]
    pub text: Vec<Text>,
    #[serde(default)]
    pub number: Vec<Text>,
    #[serde(default)]
    pub date: Vec<Text>,
    /// Patterns with `*`, such as `notiene@*`.
    #[serde(default)]
    pub email: Vec<Text>,
}

/// Another dataset, looked up by its key.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub dataset: At<String>,
    pub key: At<String>,
    /// Which cut of the other dataset: by default the same date.
    #[serde(default)]
    pub as_of: AsOf,
    /// Only the other dataset's records where this holds (its fields):
    /// `EstadoAsesor = 'ACTIVO'`. Compared as written.
    #[serde(rename = "where")]
    pub filter: Option<At<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AsOf {
    /// The other dataset's cut of the same date.
    #[default]
    SameCut,
}

/// What a field may hold given others: one row per case, edited by the
/// field's owner as a table.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CombinationTable {
    /// The field or fields each row starts from.
    pub when: At<Fields>,
    /// The field whose values the rows allow or forbid.
    pub then: At<String>,
    pub rows: Vec<At<TableRow>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TableRow {
    /// One value, several, or `*` for every other.
    pub when: OneOrMany,
    #[serde(default)]
    pub allowed: Vec<Text>,
    #[serde(default)]
    pub forbidden: Vec<Text>,
}

/// One value or several.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(untagged)]
pub enum OneOrMany {
    One(Text),
    Many(Vec<Text>),
}
