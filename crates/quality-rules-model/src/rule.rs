//! One rule: a pattern applied to a field, with its parameters (decision
//! 0033, point 1). Every parameter is optional here; which ones a pattern
//! takes is checked by lint, so a misplaced one gets a finding with its
//! position instead of a parse error.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::at::At;
use crate::model::{Fields, Text};

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Unique in the dataset; kept when a rule is imported (`V-PN-01`).
    /// A named rule has none (decision 0045).
    #[serde(default)]
    pub id: At<String>,
    /// The field it checks; a list gives one rule per field.
    pub field: Option<At<Fields>>,
    /// Repeats the rule for each value: `{ n: [1, 2] }` with `TelefonoRef{n}`.
    pub for_each: Option<At<BTreeMap<String, Vec<Text>>>>,
    /// Its pattern; a rule that `extends` a named one may take it from it.
    pub check: Option<At<Pattern>>,
    /// A named rule of the table's global rules this one starts from: its
    /// keys, then this rule's on top (decision 0045).
    pub extends: Option<At<String>>,
    /// The rule applies only where this condition holds.
    pub when: Option<At<String>>,
    /// A population declared by the dataset.
    pub population: Option<At<String>>,
    /// By default, the pattern's.
    pub severity: Option<At<Severity>>,
    pub cause: Option<At<Cause>>,
    /// What the person who fixes the record reads.
    pub alert: Option<At<String>>,
    /// Only for `expression`: the others take it from the pattern.
    pub dimension: Option<At<Dimension>>,
    /// An empty value is skipped by every pattern but `required` and
    /// `empty`; `fail` counts it as a failure instead.
    pub on_empty: Option<At<OnEmpty>>,
    /// Text comparisons are trimmed and case-insensitive by default.
    pub case: Option<At<Case>>,

    /// `placeholder`: the fillers to find (default: the dataset's).
    pub values: Option<At<Vec<Text>>>,
    /// `domain`: the allowed values; `length`: the allowed lengths.
    #[serde(rename = "in")]
    pub in_: Option<At<ValueSet>>,
    /// `domain`: the forbidden values.
    pub not_in: Option<At<ValueSet>>,
    /// `length`, `range`: a number, a date, a field, `:cut` or an expression.
    pub min: Option<At<Text>>,
    pub max: Option<At<Text>>,
    pub min_exclusive: Option<At<bool>>,
    pub max_exclusive: Option<At<bool>>,
    /// `format`: a regular expression, anchored to the whole value.
    pub pattern: Option<At<String>>,
    /// `format`: a format maintained by the platform.
    pub named: Option<At<NamedFormat>>,
    /// `fresh`: how old a date may be against the cut (`13 months`).
    pub max_age: Option<At<String>>,
    /// `sum`: the parts, each with its sign (`- Pasivo`).
    pub parts: Option<At<Vec<String>>>,
    /// `sum`: the difference allowed.
    pub tolerance: Option<At<Text>>,
    /// `unique`: the fields that must not repeat (default: the field).
    pub key: Option<At<Fields>>,
    /// `unique`, `differs`: compare normalized values.
    pub normalize: Option<At<bool>>,
    /// `unique`: values that may repeat.
    pub ignore: Option<At<Vec<Text>>>,
    /// `combination`: a table declared by the dataset.
    pub table: Option<At<String>>,
    /// `implies`: when this holds, `then` must hold.
    #[serde(rename = "if")]
    pub if_: Option<At<String>>,
    pub then: Option<At<String>>,
    /// `implies`: `then` must also imply `if`.
    pub both_ways: Option<At<bool>>,
    /// `differs`: the other field.
    pub from: Option<At<String>>,
    /// `expression`: the rule fails where this holds.
    pub fails: Option<At<String>>,

    /// Where an imported rule comes from.
    pub origin: Option<Origin>,
    pub note: Option<String>,
    pub examples: Option<At<Examples>>,
}

impl Rule {
    /// Its pattern. Every rule the compiler is given has one: `parse`
    /// refuses a rule with neither `check` nor `extends`, `with_globals`
    /// fills an extended one's, and `compile` leaves out any other.
    pub fn check(&self) -> &At<Pattern> {
        self.check
            .as_ref()
            .expect("a rule reaches the compiler with its pattern")
    }
}

/// The fourteen patterns (decision 0033, point 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    Required,
    Empty,
    Placeholder,
    Length,
    Format,
    Domain,
    Range,
    Fresh,
    Sum,
    Unique,
    Combination,
    Implies,
    Differs,
    Expression,
}

impl Pattern {
    /// Its name as the YAML writes it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Empty => "empty",
            Self::Placeholder => "placeholder",
            Self::Length => "length",
            Self::Format => "format",
            Self::Domain => "domain",
            Self::Range => "range",
            Self::Fresh => "fresh",
            Self::Sum => "sum",
            Self::Unique => "unique",
            Self::Combination => "combination",
            Self::Implies => "implies",
            Self::Differs => "differs",
            Self::Expression => "expression",
        }
    }
}

/// Values given inline, a list of the dataset, or a reference's column.
#[derive(Debug, Clone, PartialEq, JsonSchema)]
#[serde(untagged)]
pub enum ValueSet {
    Values(Vec<Text>),
    List {
        list: String,
    },
    /// `dataset.column`, a reference declared by the dataset.
    Reference {
        reference: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NamedFormat {
    Email,
    MobileCo,
    Divipola,
    AddressCo,
    PersonName,
    NotOnlySymbols,
    Capitalized,
    NormalizedSpaces,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Blocking,
    Major,
    Minor,
}

impl Severity {
    /// Its name as the YAML writes it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Blocking => "blocking",
            Self::Major => "major",
            Self::Minor => "minor",
        }
    }
}

/// Why a record fails: at capture, or in a system that moves the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Cause {
    Capture,
    Systemic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    Completeness,
    Consistency,
    Accuracy,
    Validity,
    Timeliness,
    Uniqueness,
}

impl Dimension {
    /// Its name as the YAML writes it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Completeness => "completeness",
            Self::Consistency => "consistency",
            Self::Accuracy => "accuracy",
            Self::Validity => "validity",
            Self::Timeliness => "timeliness",
            Self::Uniqueness => "uniqueness",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OnEmpty {
    Skip,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Case {
    Insensitive,
    Exact,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    /// The process it was imported from (`KNIME`).
    pub process: String,
    /// Where in that process (`"#1636"`).
    pub node: Option<String>,
}

/// Records the rule must fail, pass and skip; lint runs them.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Examples {
    #[serde(default)]
    pub fails: Records,
    #[serde(default)]
    pub passes: Records,
    #[serde(default)]
    pub skips: Records,
}

/// One record or several: field → value, `null` for no value.
#[derive(Debug, Clone, PartialEq, JsonSchema)]
#[serde(untagged)]
pub enum Records {
    One(BTreeMap<String, Option<Text>>),
    Many(Vec<BTreeMap<String, Option<Text>>>),
}

impl Default for Records {
    fn default() -> Self {
        Self::Many(Vec::new())
    }
}

impl Records {
    pub fn all(&self) -> Vec<&BTreeMap<String, Option<Text>>> {
        match self {
            Self::One(record) => vec![record],
            Self::Many(records) => records.iter().collect(),
        }
    }
}
