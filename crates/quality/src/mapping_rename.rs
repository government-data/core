//! Fields named as their columns (decision 0044): which of a dataset's
//! fields are named otherwise, and its mapping with them renamed, as text
//! (each field's key under `fields:`, and the `key:` list), the rest as
//! written. The rules that name them are renamed by
//! `quality_rules::lint::rename_fields`.

use std::collections::BTreeMap;

use crate::mapping::Mapping;
use crate::proposal::field_name;

/// Old name → new name.
pub type Renames = BTreeMap<String, String>;

/// Why the fields cannot all take their columns' names: two would have
/// one name, or one would take the name of a field that stays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clash {
    pub name: String,
}

/// Each field whose name is not its column's, and the name it takes.
pub fn renames(mapping: &Mapping) -> Result<Renames, Clash> {
    let out: Renames = mapping
        .fields
        .iter()
        .map(|(field, f)| (field.clone(), field_name(&f.value.column)))
        .filter(|(field, new)| field != new)
        .collect();
    let mut taken: Vec<&str> = mapping
        .fields
        .keys()
        .filter(|f| !out.contains_key(*f))
        .map(String::as_str)
        .collect();
    for new in out.values() {
        if taken.contains(&new.as_str()) {
            return Err(Clash { name: new.clone() });
        }
        taken.push(new);
    }
    Ok(out)
}

/// A word of `text` renamed if it is one of `renames`.
fn words(text: &str, renames: &Renames) -> String {
    let mut out = String::new();
    let mut word = String::new();
    for c in text.chars().chain(std::iter::once('\n')) {
        if c.is_ascii_alphanumeric() || c == '_' {
            word.push(c);
            continue;
        }
        out.push_str(renames.get(&word).map_or(&word, |n| n));
        word.clear();
        out.push(c);
    }
    out.pop();
    out
}

/// `yaml` (a mapping) with its fields renamed: each key under `fields:`
/// and each name of `key:`.
pub fn renamed_mapping(yaml: &str, renames: &Renames) -> String {
    let mut section = "";
    // The indent of a field's own line under `fields:`.
    let mut field_at: Option<usize> = None;
    let mut out: Vec<String> = Vec::new();
    for line in yaml.lines() {
        let top = !line.is_empty() && !line.starts_with([' ', '#']);
        if top {
            section = line.split(':').next().unwrap_or("");
        }
        let renamed = match section {
            "key" => words(line, renames),
            "fields" if !top && !line.trim().is_empty() && !line.trim_start().starts_with('#') => {
                let at = line.len() - line.trim_start().len();
                let rest = line.trim_start();
                let key_end = rest.find(':').unwrap_or(0);
                let key = rest[..key_end].trim_matches(|c| c == '"' || c == '\'');
                let first = *field_at.get_or_insert(at);
                match renames.get(key).filter(|_| first == at) {
                    Some(new) => format!("{}{new}{}", &line[..at], &rest[key_end..]),
                    None => line.to_string(),
                }
            }
            _ => line.to_string(),
        };
        out.push(renamed);
    }
    let mut text = out.join("\n");
    if yaml.ends_with('\n') {
        text.push('\n');
    }
    text
}
