//! Fields named as their columns (decision 0044): a dataset's rules files
//! with each field renamed, as text, so comments and layout stay. A name
//! is renamed where it is a field: in expressions and lists of fields, in
//! `examples`, and, for another dataset's fields, in `references` and in
//! `'alias.Field'`. Never in what people read (`alert`, `note`…), in a
//! regular expression, a list of values, a comment, or a text in single
//! quotes. Refused unless the same rules run, each reading the same fields
//! by their new names, with no error.

use std::collections::BTreeMap;

use quality_rules_compile::Schema;
use quality_rules_model::{parse, GLOBAL_FILE};

use crate::{lint_globals, lint_with, Severity};

/// Old name → new name, of one dataset's fields.
pub type Renames = BTreeMap<String, String>;

/// The keys whose values are never a field's name.
const PROSE: [&str; 28] = [
    "alert",
    "note",
    "description",
    "origin",
    "pattern",
    "named",
    "max_age",
    "values",
    "id",
    "check",
    "severity",
    "cause",
    "dimension",
    "on_empty",
    "case",
    "extends",
    "population",
    "table",
    "dataset",
    "normalize",
    "ignore",
    "list",
    "as_of",
    "rows",
    "allowed",
    "forbidden",
    "in",
    "not_in",
];

/// The keys a rules file writes: a word before `:` that is one of them is
/// a key; any other is a field (a record of `examples`).
const KEYS: [&str; 53] = [
    "id",
    "field",
    "for_each",
    "check",
    "extends",
    "when",
    "population",
    "severity",
    "cause",
    "alert",
    "dimension",
    "on_empty",
    "case",
    "values",
    "in",
    "not_in",
    "min",
    "max",
    "min_exclusive",
    "max_exclusive",
    "pattern",
    "named",
    "max_age",
    "parts",
    "tolerance",
    "key",
    "normalize",
    "ignore",
    "table",
    "if",
    "then",
    "both_ways",
    "from",
    "fails",
    "origin",
    "note",
    "examples",
    "passes",
    "skips",
    "dataset",
    "populations",
    "lists",
    "placeholders",
    "derived",
    "references",
    "tables",
    "rules",
    "where",
    "as_of",
    "rows",
    "allowed",
    "forbidden",
    "list",
];

/// The sections whose entries hold no field.
const NO_FIELDS: [&str; 2] = ["lists", "placeholders"];

/// How a line's names are renamed.
struct Names<'a> {
    own: &'a Renames,
    /// A reference's alias → its dataset's renames.
    aliases: &'a BTreeMap<String, Renames>,
}

impl Names<'_> {
    /// `line` renamed by `by`, under the key `key` (the innermost open):
    /// each name outside single quotes where the key it is under is not
    /// prose, `alias.Field` and `'alias.Field'` by the alias's dataset;
    /// nothing after a comment's `#`.
    fn line(&self, line: &str, by: &Renames, key: &str) -> String {
        let mut out = String::new();
        let mut rest = line;
        let mut key = key.to_string();
        // The word before a `.`, if that is where the text is.
        let mut last = String::new();
        while let Some(c) = rest.chars().next() {
            if c == '\'' {
                let end = rest[1..].find('\'').map_or(rest.len(), |i| i + 2);
                out.push_str(&self.quoted(&rest[..end]));
                rest = &rest[end..];
            } else if c == '#' && (out.is_empty() || out.ends_with(' ')) {
                out.push_str(rest);
                break;
            } else if c.is_ascii_alphanumeric() || c == '_' {
                let end = rest
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(rest.len());
                let word = &rest[..end];
                let is_key = rest[end..].trim_start().starts_with(':')
                    && !rest[end..].trim_start().starts_with("::")
                    && KEYS.contains(&word);
                let new = if is_key {
                    key = word.to_string();
                    None
                } else if out.ends_with('.') {
                    self.aliases.get(&last).and_then(|r| r.get(word))
                } else if out.ends_with(':') || PROSE.contains(&key.as_str()) {
                    None
                } else {
                    by.get(word)
                };
                out.push_str(new.map_or(word, String::as_str));
                last = word.to_string();
                rest = &rest[end..];
            } else {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
        out
    }

    /// `'alias.Field'` with Field renamed by the alias's dataset; any other
    /// quoted text as it is.
    fn quoted(&self, text: &str) -> String {
        let inner = text.trim_matches('\'');
        if let Some((alias, field)) = inner.split_once('.') {
            if let Some(new) = self.aliases.get(alias).and_then(|r| r.get(field)) {
                return format!("'{alias}.{new}'");
            }
        }
        text.to_string()
    }
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The key a line starts, if it starts one (`- ` before it allowed).
fn key_of(line: &str) -> Option<&str> {
    let t = line.trim_start().trim_start_matches("- ");
    let end = t.find(':')?;
    let key = &t[..end];
    (!key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).then_some(key)
}

/// One file renamed: `own` for the dataset's fields, `aliases` for those
/// of the datasets its references read.
fn file(yaml: &str, names: &Names<'_>) -> String {
    let mut section = String::new();
    // The open keys, by indent: the innermost decides.
    let mut open: Vec<(usize, String)> = Vec::new();
    let mut lines = Vec::new();
    for line in yaml.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            lines.push(line.to_string());
            continue;
        }
        let at = indent(line);
        if at == 0 {
            section = key_of(line).unwrap_or_default().to_string();
            open.clear();
        }
        open.retain(|(i, _)| *i < at);
        if let Some(key) = key_of(line) {
            let at = if line.trim_start().starts_with("- ") {
                at + 2
            } else {
                at
            };
            open.push((at, key.to_string()));
        }
        // The key the line's first value is under: the one open above it
        // (a key the line starts is read by `line` itself).
        let above = open
            .iter()
            .rev()
            .find(|(i, _)| *i < at)
            .map(|(_, k)| k.as_str())
            .unwrap_or_default();
        let renamed = if section == "references" {
            // An entry's other dataset: its `key`, `where` and fields.
            let alias = open.get(1).map(|(_, k)| k.as_str()).unwrap_or_default();
            match names.aliases.get(alias) {
                Some(by) => names.line(line, by, above),
                None => line.to_string(),
            }
        } else if NO_FIELDS.contains(&section.as_str()) {
            line.to_string()
        } else {
            names.line(line, names.own, above)
        };
        lines.push(renamed);
    }
    let mut out = lines.join("\n");
    if yaml.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Each reference's alias and the dataset it reads, in `files`.
pub fn references(files: &[(String, String)]) -> BTreeMap<String, String> {
    files
        .iter()
        .filter_map(|(_, y)| parse(y).ok())
        .flat_map(|f| f.references.into_iter())
        .map(|(alias, r)| (alias, r.dataset.value))
        .collect()
}

/// What a dataset's files run: each rule's id and the fields it reads.
fn runs(files: &[(String, String)], schema: &Schema) -> Option<BTreeMap<String, Vec<String>>> {
    let globals = files
        .iter()
        .find(|(f, _)| f == GLOBAL_FILE)
        .map(|(_, y)| y.as_str());
    let mut out = BTreeMap::new();
    for (name, yaml) in files {
        let report = if name == GLOBAL_FILE {
            lint_globals(yaml, schema)
        } else {
            lint_with(yaml, schema, globals)
        };
        if report
            .findings
            .iter()
            .any(|f| f.severity() == Severity::Error)
        {
            return None;
        }
        for c in report.compiled {
            let mut reads: Vec<String> = c.reads().into_iter().collect();
            reads.sort();
            out.insert(c.id.clone(), reads);
        }
    }
    Some(out)
}

/// `files` with the dataset's fields renamed by `own` (against `schema`
/// before and `renamed` after) and its references' by `others` (dataset →
/// renames); `None` if the result would not run the same rules on the same
/// fields, or the files had errors already.
pub fn rename_fields(
    files: &[(String, String)],
    (schema, renamed): (&Schema, &Schema),
    own: &Renames,
    others: &BTreeMap<String, Renames>,
) -> Option<Vec<(String, String)>> {
    let before = runs(files, schema)?;
    let aliases: BTreeMap<String, Renames> = references(files)
        .into_iter()
        .filter_map(|(alias, dataset)| others.get(&dataset).map(|r| (alias, r.clone())))
        .collect();
    let names = Names {
        own,
        aliases: &aliases,
    };
    let out: Vec<(String, String)> = files
        .iter()
        .map(|(f, y)| (f.clone(), file(y, &names)))
        .collect();
    let after = runs(&out, renamed)?;
    // A rule over several fields runs as `id.Field`: its id follows.
    let renamed_id = |id: String| -> String {
        id.split('.')
            .enumerate()
            .map(|(i, part)| match own.get(part) {
                Some(new) if i > 0 => new.as_str(),
                _ => part,
            })
            .collect::<Vec<_>>()
            .join(".")
    };
    let expected: BTreeMap<String, Vec<String>> = before
        .into_iter()
        .map(|(id, reads)| {
            let mut reads: Vec<String> = reads
                .into_iter()
                .map(|r| own.get(&r).cloned().unwrap_or(r))
                .collect();
            reads.sort();
            (renamed_id(id), reads)
        })
        .collect();
    (after == expected).then_some(out)
}
