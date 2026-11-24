//! What a map job proposes for a table (decisions 0020 and 0038): from the
//! table's metadata and its sample's aggregates (never a value), each
//! column's silver and semantic type and its roles
//! ([`crate::proposal_roles`]), the dataset's key, and rules by pattern. Pure: the worker measures, the application writes the draft.

use serde::{Deserialize, Serialize};

use crate::proposal_roles::{expires, Roles};

/// What a value must look like to count as each kind, as the profile
/// measures it and as the proposed format rules check it (anchored, 0033).
/// An installation sets its own (`QUALITY_SEMANTIC_PATTERNS`); the defaults
/// are Colombia's. A map job carries the ones it was queued with, so its
/// profile and its proposal use the same.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Patterns {
    pub email: String,
    /// Colombia: a 3 and nine digits.
    pub mobile: String,
    /// Colombia: a cédula or a NIT, five to twelve digits.
    pub document: String,
    pub date: String,
    pub number: String,
    pub flag: String,
}

impl Default for Patterns {
    fn default() -> Self {
        Self {
            email: r"[^@\s]+@[^@\s]+\.[A-Za-z]{2,}".into(),
            mobile: r"3\d{9}".into(),
            document: r"\d{5,12}".into(),
            date: r"\d{4}-\d{2}-\d{2}.*".into(),
            number: r"-?\d+([.,]\d+)?".into(),
            flag: r"(?i)(s|n|si|no|y|yes|true|false|0|1)".into(),
        }
    }
}

impl Patterns {
    /// Each pattern by its kind, in [`Looks`]' order.
    pub fn all(&self) -> [(&'static str, &str); 6] {
        [
            ("email", &self.email),
            ("mobile", &self.mobile),
            ("document", &self.document),
            ("date", &self.date),
            ("number", &self.number),
            ("flag", &self.flag),
        ]
    }

    /// The ones a map job was queued with, or the defaults.
    pub fn of_job(parameters: &serde_json::Value) -> Self {
        parameters
            .get("patterns")
            .and_then(|p| serde_json::from_value(p.clone()).ok())
            .unwrap_or_default()
    }
}

/// A share of the non-empty values at least this big is what a column is.
const MOSTLY: f64 = 0.95;
/// Longer than this, a text is free text.
const FREE_TEXT: u32 = 60;

/// How many of a column's non-empty sampled values look like each kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Looks {
    pub email: u64,
    pub mobile: u64,
    pub document: u64,
    pub date: u64,
    pub number: u64,
    pub flag: u64,
}

/// One column: its metadata and its sample's aggregates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnProfile {
    pub name: String,
    /// As the engine reads it (`VARCHAR`, `INTEGER`, `DECIMAL(14,2)`…).
    pub declared_type: String,
    pub nullable: bool,
    pub primary_key: bool,
    /// Sampled values that are empty (`NULL`, or blank, decision 0033).
    pub empty: u64,
    pub distinct: u64,
    pub min_length: Option<u32>,
    pub max_length: Option<u32>,
    pub looks: Looks,
}

/// A table as a map job saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableProfile {
    /// The source's name, and the table as the source names it.
    pub source: String,
    pub table: String,
    pub row_estimate: Option<i64>,
    /// Rows in the sample.
    pub sampled: u64,
    pub columns: Vec<ColumnProfile>,
}

/// The texts the proposed rules carry, in the installation's language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alerts {
    pub required: String,
    pub unique: String,
    pub email: String,
    pub mobile: String,
    pub document: String,
}

/// A dataset's proposal: its mapping and its rules files, as YAML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub mapping: String,
    pub rules: Vec<(String, String)>,
}

/// The silver and semantic types of a column.
fn types(c: &ColumnProfile, sampled: u64) -> (&'static str, Option<&'static str>) {
    let filled = sampled.saturating_sub(c.empty);
    let mostly = |n: u64| filled > 0 && n as f64 / filled as f64 >= MOSTLY;
    let declared = c.declared_type.to_ascii_uppercase();
    let numeric = ["INT", "DECIMAL", "NUMERIC", "DOUBLE", "FLOAT", "REAL"]
        .iter()
        .any(|t| declared.contains(t));
    if declared.contains("BOOL") {
        return ("boolean", Some("flag"));
    }
    if declared.starts_with("DATE") || declared.starts_with("TIMESTAMP") {
        return ("date", None);
    }
    if mostly(c.looks.email) {
        return ("text", Some("email"));
    }
    if mostly(c.looks.mobile) {
        return ("text", Some("mobile"));
    }
    if numeric && !c.primary_key {
        return ("number", Some("amount"));
    }
    if mostly(c.looks.document) && c.distinct * 10 >= filled * 9 {
        return ("text", Some("document"));
    }
    if c.distinct <= 2 && mostly(c.looks.flag) {
        return ("text", Some("flag"));
    }
    if c.max_length.is_some_and(|l| l > FREE_TEXT) {
        return ("text", Some("free_text"));
    }
    ("text", (c.distinct * 2 <= filled).then_some("code"))
}

/// A field's name from its column's: the column's own when it is a name
/// (decision 0044: a field is named as its column), or else made one
/// (`client id` → `ClientId`).
pub(crate) fn field_name(column: &str) -> String {
    let named = column
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && column
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    if named {
        return column.to_string();
    }
    let name: String = column
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut chars = p.chars();
            chars.next().map_or(String::new(), |f| {
                f.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect();
    match name.chars().next() {
        Some(c) if c.is_ascii_alphabetic() => name,
        _ => format!("F{name}"),
    }
}

/// A YAML string, double-quoted.
fn quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

/// The dataset `dataset`'s mapping and rules for `profile`.
pub fn propose(
    dataset: &str,
    profile: &TableProfile,
    patterns: &Patterns,
    alerts: &Alerts,
) -> Proposal {
    let mut fields = Vec::new();
    let mut key = Vec::new();
    let mut rules = Vec::new();
    let mut proposed = Roles::default();
    for c in &profile.columns {
        let field = field_name(&c.name);
        let (kind, semantic) = types(c, profile.sampled);
        let roles = proposed.of(c, kind, semantic, profile.sampled);
        let roles = if roles.is_empty() {
            String::new()
        } else {
            format!(", roles: [{}]", roles.join(", "))
        };
        let semantic = semantic.map_or(String::new(), |s| format!(", semantic: {s}"));
        let expiry = if expires(&c.name) {
            ", expires: true"
        } else {
            ""
        };
        fields.push(format!(
            "  {field}: {{ column: {}, type: {kind}{semantic}{roles}{expiry} }}",
            quoted(&c.name)
        ));
        if c.primary_key {
            key.push(field.clone());
        }
        let mut rule = |check: String, alert: &str| {
            let text = format!(
                "  - id: \"MAP-{:03}\"\n    field: {}\n{check}    severity: major\n    cause: capture\n    alert: {}\n",
                rules.len() + 1,
                quoted(&field),
                quoted(alert)
            );
            rules.push((field.clone(), text));
        };
        if c.empty == 0 && !c.nullable {
            rule("    check: \"required\"\n".into(), &alerts.required);
        }
        if c.primary_key {
            rule("    check: \"unique\"\n".into(), &alerts.unique);
        }
        let format = match semantic.trim_start_matches(", semantic: ") {
            "email" => Some((patterns.email.as_str(), &alerts.email)),
            "mobile" => Some((patterns.mobile.as_str(), &alerts.mobile)),
            "document" => Some((patterns.document.as_str(), &alerts.document)),
            _ => None,
        };
        if let Some((pattern, alert)) = format {
            rule(
                format!("    check: \"format\"\n    pattern: {}\n", quoted(pattern)),
                alert,
            );
        }
    }
    let key = if key.is_empty() {
        String::new()
    } else {
        format!("key: [{}]\n", key.join(", "))
    };
    let mapping = format!(
        "dataset: {dataset}\nsource: {}\nfrom: {{ source: {}, table: {} }}\n{key}fields:\n{}\n",
        table_name(&profile.table),
        profile.source,
        quoted(&profile.table),
        fields.join("\n")
    );
    // A field's rules in its own file, in the columns' order (decision 0044).
    let mut files: Vec<(String, Vec<String>)> = Vec::new();
    for (field, text) in rules {
        match files.iter_mut().find(|(f, _)| *f == field) {
            Some((_, texts)) => texts.push(text),
            None => files.push((field, vec![text])),
        }
    }
    let files = files
        .into_iter()
        .map(|(field, texts)| {
            (
                format!("{field}.yaml"),
                format!("dataset: {dataset}\nrules:\n{}", texts.join("\n")),
            )
        })
        .collect();
    Proposal {
        mapping,
        rules: files,
    }
}

/// The bronze table's name: the source table's last part, lowercase.
pub(crate) fn table_name(table: &str) -> String {
    let last = table.rsplit('.').next().unwrap_or(table);
    last.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}
