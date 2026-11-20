//! _Map the whole table_ (decision 0043): the columns no field reads,
//! added to a dataset's mapping as fields of their own, as text at the end
//! of `fields:` so the rest of the file stays as it was written; a table no
//! dataset reads starts one.

use crate::mapping::Mapping;
use crate::proposal::{field_name, table_name};

/// Why a column was not added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotAdded {
    /// A field reads it already.
    Read,
    /// The field it would be is there, reading another column.
    Taken,
    /// The mapping does not read, or has no `fields:` block to add to.
    Mapping,
}

/// A field's type in silver from its column's type in the source.
pub fn kind_of(declared: &str) -> &'static str {
    let declared = declared.to_ascii_uppercase();
    let numeric = ["INT", "DECIMAL", "NUMERIC", "DOUBLE", "FLOAT", "REAL"];
    if declared.contains("BOOL") {
        "boolean"
    } else if declared.starts_with("DATE") || declared.starts_with("TIMESTAMP") {
        "date"
    } else if numeric.iter().any(|t| declared.contains(t)) {
        "number"
    } else {
        "text"
    }
}

/// `yaml` with `column` read as a new field, and the field's name.
pub fn with_field(yaml: &str, column: &str, declared: &str) -> Result<(String, String), NotAdded> {
    let mapping = Mapping::parse(yaml).map_err(|_| NotAdded::Mapping)?;
    if mapping.fields.values().any(|f| f.value.column == column) {
        return Err(NotAdded::Read);
    }
    let field = field_name(column);
    if mapping.fields.contains_key(&field) {
        return Err(NotAdded::Taken);
    }
    let lines: Vec<&str> = yaml.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim_end() == "fields:")
        .ok_or(NotAdded::Mapping)?;
    // The block runs while its lines are indented (or blank); the field
    // goes after its last line that says something.
    let mut end = start + 1;
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        if !line.trim().is_empty() && !line.starts_with([' ', '\t']) {
            break;
        }
        if !line.trim().is_empty() {
            end = i + 1;
        }
    }
    let first = lines[start + 1..end].iter().find(|l| !l.trim().is_empty());
    let indent = first.map_or("  ", |l| &l[..l.len() - l.trim_start().len()]);
    let quoted = serde_json::to_string(column).unwrap_or_default();
    let line = format!(
        "{indent}{field}: {{ column: {quoted}, type: {} }}",
        kind_of(declared)
    );
    let mut out: Vec<&str> = lines[..end].to_vec();
    out.push(&line);
    out.extend_from_slice(&lines[end..]);
    let text = out.join("\n") + "\n";
    match Mapping::parse(&text) {
        Ok(m) if m.fields.contains_key(&field) => Ok((text, field)),
        _ => Err(NotAdded::Mapping),
    }
}

/// `yaml` with each of `columns` (name, type in the source) a field reads
/// not yet read as a new field, and the fields added; the first column
/// that cannot be added, and why.
pub fn with_columns(
    mut yaml: String,
    columns: &[(&str, &str)],
) -> Result<(String, Vec<String>), (NotAdded, String)> {
    let mut fields = Vec::new();
    for (column, declared) in columns {
        match with_field(&yaml, column, declared) {
            Ok((text, field)) => {
                yaml = text;
                fields.push(field);
            }
            Err(NotAdded::Read) => {}
            Err(why) => return Err((why, column.to_string())),
        }
    }
    Ok((yaml, fields))
}

/// The table a dataset is started from: a source's (`schema.table`) or,
/// with no source, the lake's own, as its bronze namespace holds it.
#[derive(Debug, Clone, Copy)]
pub struct Start<'a> {
    pub source: Option<&'a str>,
    pub table: &'a str,
}

/// A new mapping for `dataset` that reads `start` with `column` as its one
/// field, and the field's name.
pub fn started(
    dataset: &str,
    start: Start<'_>,
    column: &str,
    declared: &str,
) -> Result<(String, String), NotAdded> {
    let quoted = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let (bronze, from) = match start.source {
        Some(source) => (
            table_name(start.table),
            format!(
                "from: {{ source: {source}, table: {} }}\n",
                quoted(start.table)
            ),
        ),
        None => (start.table.to_string(), String::new()),
    };
    let field = field_name(column);
    let text = format!(
        "dataset: {dataset}\nsource: {}\n{from}fields:\n  {field}: {{ column: {}, type: {} }}\n",
        quoted(&bronze),
        quoted(column),
        kind_of(declared)
    );
    match Mapping::parse(&text) {
        Ok(_) => Ok((text, field)),
        Err(_) => Err(NotAdded::Mapping),
    }
}
