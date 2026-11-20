//! The keys a record is known by, as SQL over its fields: its entity's
//! (decision 0022) and its own (decision 0059), both normalized the same
//! way, so a document written two ways is one key.

use quality::mapping::{FieldRole, Mapping, Semantic};

use crate::{quoted, Error};

/// `names`' values normalized (a document's digits without leading zeros,
/// any other trimmed and upper case) and joined by `|`, `NULL` if one is
/// empty; `None` without names.
fn normalized<'a>(
    mapping: &Mapping,
    names: impl Iterator<Item = &'a String>,
) -> Result<Option<String>, Error> {
    let mut parts = Vec::new();
    for name in names {
        let document = mapping
            .fields
            .get(name)
            .is_some_and(|f| f.value.semantic == Some(Semantic::Document));
        let v = format!("CAST({} AS VARCHAR)", quoted(name)?);
        parts.push(if document {
            format!("NULLIF(ltrim(regexp_replace({v}, '[^0-9]', '', 'g'), '0'), '')")
        } else {
            format!("NULLIF(upper(trim({v})), '')")
        });
    }
    Ok((!parts.is_empty()).then(|| parts.join(" || '|' || ")))
}

fn entity_fields(mapping: &Mapping) -> impl Iterator<Item = &String> {
    mapping
        .fields
        .iter()
        .filter(|(_, f)| f.value.roles.contains(&FieldRole::Entity))
        .map(|(name, _)| name)
}

/// The entity's key: its fields normalized.
pub(crate) fn entity_key(mapping: &Mapping) -> Result<Option<String>, Error> {
    normalized(mapping, entity_fields(mapping))
}

/// A record's own key: the mapping's `key` fields normalized, or else its
/// entity's; `None` when the dataset has neither.
pub(crate) fn record_key(mapping: &Mapping) -> Result<Option<String>, Error> {
    if mapping.key.is_empty() {
        entity_key(mapping)
    } else {
        normalized(mapping, mapping.key.iter())
    }
}
