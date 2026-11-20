//! _Split by field_ (decisions 0044 and 0045): a dataset's rules files
//! rewritten as its table's global rules and a file per field. What the
//! files declare goes to `reglas_globales.yaml`, each rule with its
//! comments to its field's file (its `field`, the first of a list, or else
//! the first field it reads), in the order the files had them; files left
//! with nothing are removed. Refused, with why, when a file does not read
//! or has errors, a declaration is written inline, two files declare one
//! name differently, a rule has no field, or the result would not lint
//! with no error or would run another number of rules.

use std::collections::BTreeMap;

use quality_rules_compile::Schema;
use quality_rules_model::{parse, Fields, GLOBAL_FILE};

use crate::blocks::{blocks, same, sections, Block};
use crate::{lint_globals, lint_with, Severity};

/// The declarations, in the order the global rules write them.
const DECLARATIONS: [&str; 7] = [
    "populations",
    "lists",
    "placeholders",
    "derived",
    "references",
    "tables",
    "named",
];

/// Why a dataset's files were not split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SplitRefused {
    /// A file does not read, or lints with errors: fix it first.
    Errors { file: String },
    /// A section written on its key's line (`lists: {a: [1]}`).
    Inline { file: String, key: String },
    /// Two files declare one name differently.
    Conflict { kind: String, name: String },
    /// A rule whose field could not be told.
    NoField { file: String, rule: String },
    /// The files as split would not lint, or would run other rules.
    Lost,
}

/// A file's new text, or `None` to remove it.
pub type Change = (String, Option<String>);

/// The changes that split `files` (name, text) by field; none if they
/// already are.
pub fn split_by_field(
    files: &[(String, String)],
    schema: &Schema,
) -> Result<Vec<Change>, SplitRefused> {
    let globals = files
        .iter()
        .find(|(f, _)| f == GLOBAL_FILE)
        .map(|(_, y)| y.as_str());
    let before = rules_run(files, schema, globals)?;
    let dataset = files
        .iter()
        .find_map(|(_, y)| parse(y).ok().map(|f| f.dataset.value))
        .unwrap_or_default();

    let mut declared: BTreeMap<&str, Vec<Block>> = BTreeMap::new();
    let mut by_field: Vec<(String, Vec<Block>)> = Vec::new();
    let mut preambles: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // The global rules first, so what they declare stays first.
    let ordered = files
        .iter()
        .filter(|(f, _)| f == GLOBAL_FILE)
        .chain(files.iter().filter(|(f, _)| f != GLOBAL_FILE));
    for (file, yaml) in ordered {
        let (preamble, parts) = sections(yaml);
        preambles.insert(file.clone(), preamble);
        for part in &parts {
            if part.key == "dataset" {
                continue;
            }
            let empty = matches!(part.inline.as_str(), "" | "[]" | "{}");
            if !empty {
                return Err(SplitRefused::Inline {
                    file: file.clone(),
                    key: part.key.clone(),
                });
            }
            if part.key == "rules" {
                let items = blocks(&part.body, true);
                for (item, field) in items
                    .into_iter()
                    .zip(fields_of(file, yaml, schema, globals)?)
                {
                    match by_field.iter_mut().find(|(f, _)| *f == field) {
                        Some((_, items)) => items.push(item),
                        None => by_field.push((field, vec![item])),
                    }
                }
                continue;
            }
            let Some(kind) = DECLARATIONS.iter().find(|k| **k == part.key) else {
                return Err(SplitRefused::Inline {
                    file: file.clone(),
                    key: part.key.clone(),
                });
            };
            let into = declared.entry(kind).or_default();
            for entry in blocks(&part.body, false) {
                match into.iter().find(|e| e.name == entry.name) {
                    Some(e) if same(e, &entry) => {}
                    Some(_) => {
                        return Err(SplitRefused::Conflict {
                            kind: kind.to_string(),
                            name: entry.name,
                        })
                    }
                    None => into.push(entry),
                }
            }
        }
    }

    let out = written(
        &dataset,
        &preambles,
        &declared,
        &by_field,
        globals.is_some(),
    );
    let after = rules_run(
        &out,
        schema,
        out.iter()
            .find(|(f, _)| f == GLOBAL_FILE)
            .map(|(_, y)| y.as_str()),
    )
    .map_err(|_| SplitRefused::Lost)?;
    if after != before {
        return Err(SplitRefused::Lost);
    }
    Ok(changes(files, &out))
}

/// The files as split: the global rules with every declaration, then a
/// file per field with its rules one blank line apart.
fn written(
    dataset: &str,
    preambles: &BTreeMap<String, Vec<String>>,
    declared: &BTreeMap<&str, Vec<Block>>,
    by_field: &[(String, Vec<Block>)],
    had_globals: bool,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut global = text_head(preambles.get(GLOBAL_FILE), dataset);
    for kind in DECLARATIONS {
        if let Some(entries) = declared.get(kind).filter(|e| !e.is_empty()) {
            global.push_str(&format!("\n{kind}:\n"));
            for entry in entries {
                global.push_str(&entry.lines.join("\n"));
                global.push('\n');
            }
        }
    }
    if had_globals || !declared.values().all(Vec::is_empty) {
        out.push((GLOBAL_FILE.to_string(), global));
    }
    for (field, items) in by_field {
        let file = format!("{field}.yaml");
        let mut text = text_head(preambles.get(&file), dataset);
        text.push_str("rules:\n");
        let rules: Vec<String> = items.iter().map(|i| i.lines.join("\n")).collect();
        text.push_str(&rules.join("\n\n"));
        text.push('\n');
        out.push((file, text));
    }
    out
}

/// What changes from `files` to `out`: each file new or rewritten, and
/// each file left out, removed.
pub(crate) fn changes(files: &[(String, String)], out: &[(String, String)]) -> Vec<Change> {
    let mut changes: Vec<Change> = out
        .iter()
        .filter(|(f, y)| files.iter().all(|(g, z)| g != f || z != y))
        .map(|(f, y)| (f.clone(), Some(y.clone())))
        .collect();
    changes.extend(
        files
            .iter()
            .filter(|(f, _)| out.iter().all(|(g, _)| g != f))
            .map(|(f, _)| (f.clone(), None)),
    );
    changes
}

/// A file's head: its comments before `dataset:`, then `dataset:`.
fn text_head(preamble: Option<&Vec<String>>, dataset: &str) -> String {
    let mut text: String = preamble.map_or(String::new(), |p| {
        p.iter().map(|l| format!("{l}\n")).collect()
    });
    text.push_str(&format!("dataset: {dataset}\n"));
    text
}

/// Each rule's field, in the file's order: its `field` (the first of a
/// list), or else the first field it reads.
fn fields_of(
    file: &str,
    yaml: &str,
    schema: &Schema,
    globals: Option<&str>,
) -> Result<Vec<String>, SplitRefused> {
    let parsed = parse(yaml).map_err(|_| SplitRefused::Errors {
        file: file.to_string(),
    })?;
    let report = lint_with(yaml, schema, globals);
    parsed
        .rules
        .iter()
        .map(|rule| {
            let id = &rule.id.value;
            let written = rule.field.as_ref().map(|f| match &f.value {
                Fields::One(name) => name.clone(),
                Fields::Many(names) => names.first().cloned().unwrap_or_default(),
            });
            written
                .filter(|f| !f.is_empty())
                .or_else(|| {
                    report
                        .compiled
                        .iter()
                        .find(|c| c.id == *id || c.id.starts_with(&format!("{id}.")))
                        .and_then(|c| c.reads().into_iter().next())
                })
                .ok_or_else(|| SplitRefused::NoField {
                    file: file.to_string(),
                    rule: id.clone(),
                })
        })
        .collect()
}

/// The ids of the rules `files` run, sorted; refused if one has errors.
pub(crate) fn rules_run(
    files: &[(String, String)],
    schema: &Schema,
    globals: Option<&str>,
) -> Result<Vec<String>, SplitRefused> {
    let mut ids = Vec::new();
    for (file, yaml) in files {
        let report = if file == GLOBAL_FILE {
            lint_globals(yaml, schema)
        } else {
            lint_with(yaml, schema, globals)
        };
        if report
            .findings
            .iter()
            .any(|f| f.severity() == Severity::Error)
        {
            return Err(SplitRefused::Errors { file: file.clone() });
        }
        ids.extend(report.compiled.into_iter().map(|c| c.id));
    }
    ids.sort();
    Ok(ids)
}
