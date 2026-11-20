//! _Split by field_'s second pass (decisions 0044 and 0045): a rule over
//! several fields, or repeated with `for_each`, is written once per field
//! in that field's file, with the id it already ran with (`V-01.Name`); a
//! pattern two or more fields check alike becomes a named rule of the
//! table's global rules, which each of them `extends`, keeping its own
//! `alert`. Text in, text out: what is not moved keeps its comments.

use std::collections::BTreeMap;

use quality_rules_compile::Schema;
use quality_rules_model::{parse, Rule, GLOBAL_FILE};

use crate::blocks::{blocks, sections};
use crate::split::{changes, rules_run, split_by_field, Change, SplitRefused};

/// The changes that split `files` by field (`split_by_field`), then write
/// each rule over several fields per field and name the patterns fields
/// share; none if there is nothing to do. Refused as the split is, or
/// `Lost` if the result would not run the very same rules.
pub fn split_and_extract(
    files: &[(String, String)],
    schema: &Schema,
) -> Result<Vec<Change>, SplitRefused> {
    let globals_of = |files: &[(String, String)]| {
        files
            .iter()
            .find(|(f, _)| f == GLOBAL_FILE)
            .map(|(_, y)| y.clone())
    };
    let before = rules_run(files, schema, globals_of(files).as_deref())?;
    let mut split: Vec<(String, String)> = files.to_vec();
    for (file, text) in split_by_field(files, schema)? {
        split.retain(|(f, _)| *f != file);
        if let Some(text) = text {
            split.push((file, text));
        }
    }
    let dataset = files
        .iter()
        .find_map(|(_, y)| parse(y).ok().map(|f| f.dataset.value))
        .unwrap_or_default();
    let out = extract(&split, &dataset).ok_or(SplitRefused::Lost)?;
    let after =
        rules_run(&out, schema, globals_of(&out).as_deref()).map_err(|_| SplitRefused::Lost)?;
    if after != before {
        return Err(SplitRefused::Lost);
    }
    Ok(changes(files, &out))
}

/// A rule's text: the comments above it, and its keys, each with what is
/// indented under it (four spaces in, as `blocks` leaves a rule).
struct Keys {
    comments: Vec<String>,
    keys: Vec<(String, Vec<String>)>,
}

fn keys_of(lines: &[String]) -> Keys {
    let first = lines
        .iter()
        .position(|l| l.starts_with("  - "))
        .unwrap_or(0);
    let mut keys: Vec<(String, Vec<String>)> = Vec::new();
    for (i, line) in lines.iter().enumerate().skip(first) {
        let line = if i == first {
            format!("    {}", &line[4..])
        } else {
            line.clone()
        };
        let starts = line.len() - line.trim_start().len() == 4
            && !line.trim_start().starts_with('#')
            && !line.trim().is_empty();
        match keys.last_mut() {
            Some((_, chunk)) if !starts => chunk.push(line),
            _ => {
                let key = line.trim_start().split(':').next().unwrap_or("");
                keys.push((key.trim().to_string(), vec![line]));
            }
        }
    }
    Keys {
        comments: lines[..first].to_vec(),
        keys,
    }
}

/// One rule as it runs: its id, field and the text of its keys but `id`,
/// `field` and `for_each`, each `{name}` of `for_each` replaced.
struct Unit {
    id: String,
    field: String,
    comments: Vec<String>,
    body: Vec<(String, Vec<String>)>,
    /// Kept as written: one field, no `for_each`.
    whole: Option<Vec<String>>,
}

impl Unit {
    /// What it checks: its keys but `alert`, as text.
    fn pattern(&self) -> Option<String> {
        self.body.iter().any(|(k, _)| k == "check").then(|| {
            self.body
                .iter()
                .filter(|(k, _)| k != "alert")
                .flat_map(|(_, lines)| lines.iter().map(|l| l.trim().to_string()))
                .collect::<Vec<_>>()
                .join("\n")
        })
    }
}

fn quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

/// The units of a rule, as the compiler names them (`expand`): one per
/// value of `for_each` and per field, `id.Field` when there are several.
fn units(rule: &Rule, lines: &[String]) -> Vec<Unit> {
    let Keys { comments, keys } = keys_of(lines);
    let names: Vec<String> = rule.field.as_ref().map_or_else(Vec::new, |f| {
        f.value.names().into_iter().map(str::to_string).collect()
    });
    let mut combos: Vec<Vec<(String, String)>> = vec![Vec::new()];
    if let Some(each) = &rule.for_each {
        for (name, values) in &each.value {
            combos = combos
                .into_iter()
                .flat_map(|c| {
                    values.iter().map(move |v| {
                        let mut c = c.clone();
                        c.push((name.clone(), v.0.clone()));
                        c
                    })
                })
                .collect();
        }
    }
    let whole = names.len() == 1 && rule.for_each.is_none();
    let body: Vec<(String, Vec<String>)> = keys
        .into_iter()
        .filter(|(k, _)| !matches!(k.as_str(), "id" | "field" | "for_each"))
        .collect();
    let mut out = Vec::new();
    for vars in &combos {
        let subst = |text: &str| {
            vars.iter().fold(text.to_string(), |acc, (n, v)| {
                acc.replace(&format!("{{{n}}}"), v)
            })
        };
        for name in &names {
            let mut id = rule.id.value.clone();
            if names.len() > 1 {
                id = format!("{id}.{name}");
            }
            for (_, value) in vars {
                id = format!("{id}.{value}");
            }
            out.push(Unit {
                id,
                field: subst(name),
                comments: if out.is_empty() {
                    comments.clone()
                } else {
                    Vec::new()
                },
                body: body
                    .iter()
                    .map(|(k, ls)| (k.clone(), ls.iter().map(|l| subst(l)).collect()))
                    .collect(),
                whole: whole.then(|| lines.to_vec()),
            });
        }
    }
    out
}

/// A unit written as a rule: whole if it was, `extends` the named rule
/// `named` if its pattern is shared, or else its own keys.
fn written(unit: &Unit, named: Option<&str>) -> String {
    if let (Some(lines), None) = (&unit.whole, named) {
        return lines.join("\n");
    }
    let mut lines = unit.comments.clone();
    lines.push(format!("  - id: {}", quoted(&unit.id)));
    lines.push(format!("    field: {}", quoted(&unit.field)));
    match named {
        Some(name) => {
            lines.push(format!("    extends: {}", quoted(name)));
            for (_, chunk) in unit.body.iter().filter(|(k, _)| k == "alert") {
                lines.extend(chunk.iter().cloned());
            }
        }
        None => {
            for (_, chunk) in &unit.body {
                lines.extend(chunk.iter().cloned());
            }
        }
    }
    lines.join("\n")
}

/// `files` (split by field) with their rules over several fields written
/// per field and their shared patterns named in the global rules; `None`
/// if a file does not read.
pub(crate) fn extract(files: &[(String, String)], dataset: &str) -> Option<Vec<(String, String)>> {
    let globals = files.iter().find(|(f, _)| f == GLOBAL_FILE);
    let taken: Vec<String> = match globals {
        Some((_, y)) => parse(y).ok()?.named.into_keys().collect(),
        None => Vec::new(),
    };
    // Every unit, by the file it was in; the files' heads kept.
    let mut units_in: Vec<(String, Vec<Unit>)> = Vec::new();
    let mut heads: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (file, yaml) in files.iter().filter(|(f, _)| f != GLOBAL_FILE) {
        let parsed = parse(yaml).ok()?;
        let (preamble, parts) = sections(yaml);
        if parts.iter().any(|p| p.key != "dataset" && p.key != "rules") {
            return None;
        }
        heads.insert(file.clone(), preamble);
        let body = parts
            .iter()
            .find(|p| p.key == "rules")
            .map(|p| p.body.clone());
        let items = blocks(&body.unwrap_or_default(), true);
        let mut all = Vec::new();
        for (rule, item) in parsed.rules.iter().zip(&items) {
            let plain = rule.extends.is_none() && rule.field.is_some();
            let mut found = if plain {
                units(rule, &item.lines)
            } else {
                Vec::new()
            };
            if found.is_empty() {
                found.push(Unit {
                    id: rule.id.value.clone(),
                    field: file.trim_end_matches(".yaml").to_string(),
                    comments: Vec::new(),
                    body: Vec::new(),
                    whole: Some(item.lines.clone()),
                });
            }
            all.extend(found);
        }
        units_in.push((file.clone(), all));
    }
    let (names, entries) = named_patterns(&units_in, taken);
    // Each unit to its field's file.
    let mut by_file: Vec<(String, Vec<String>)> = Vec::new();
    for unit in units_in.iter().flat_map(|(_, us)| us) {
        let file = format!("{}.yaml", unit.field);
        let text = written(
            unit,
            unit.pattern()
                .and_then(|p| names.get(&p))
                .map(String::as_str),
        );
        match by_file.iter_mut().find(|(f, _)| *f == file) {
            Some((_, rules)) => rules.push(text),
            None => by_file.push((file, vec![text])),
        }
    }
    let mut out = Vec::new();
    let mut global = globals.map_or_else(|| format!("dataset: {dataset}\n"), |(_, y)| y.clone());
    if !entries.is_empty() {
        global = with_named(&global, &entries.join("\n"));
    }
    if globals.is_some() || !entries.is_empty() {
        out.push((GLOBAL_FILE.to_string(), global));
    }
    for (file, rules) in by_file {
        let mut text: String = heads.get(&file).map_or(String::new(), |p| {
            p.iter().map(|l| format!("{l}\n")).collect()
        });
        text.push_str(&format!("dataset: {dataset}\nrules:\n"));
        text.push_str(&rules.join("\n\n"));
        text.push('\n');
        out.push((file, text));
    }
    Some(out)
}

/// `yaml` (the global rules) with `entries` at the end of `named:`, the
/// section made at the end if it is not there.
fn with_named(yaml: &str, entries: &str) -> String {
    let lines: Vec<&str> = yaml.lines().collect();
    let top = |l: &str| !l.is_empty() && !l.starts_with([' ', '#']);
    let Some(start) = lines.iter().position(|l| l.trim_end() == "named:") else {
        return format!("{}\n\nnamed:\n{entries}\n", yaml.trim_end());
    };
    let mut end = lines[start + 1..]
        .iter()
        .position(|l| top(l))
        .map_or(lines.len(), |i| start + 1 + i);
    while end > start + 1 && !lines[end - 1].starts_with(' ') {
        end -= 1;
    }
    let mut out: Vec<&str> = lines[..end].to_vec();
    out.push(entries);
    out.extend_from_slice(&lines[end..]);
    out.join("\n") + "\n"
}

/// The patterns two units or more share, each named after the first rule
/// that has it (a name `taken` gets `-2`, `-3`…), and their entries for
/// `named:`.
fn named_patterns(
    units_in: &[(String, Vec<Unit>)],
    mut taken: Vec<String>,
) -> (BTreeMap<String, String>, Vec<String>) {
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    for unit in units_in.iter().flat_map(|(_, us)| us) {
        if let Some(p) = unit.pattern() {
            *count.entry(p).or_default() += 1;
        }
    }
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut entries: Vec<String> = Vec::new();
    for unit in units_in.iter().flat_map(|(_, us)| us) {
        let Some(p) = unit.pattern().filter(|p| count[p] > 1) else {
            continue;
        };
        if names.contains_key(&p) {
            continue;
        }
        let base = unit.id.split('.').next().unwrap_or(&unit.id).to_string();
        let mut name = base.clone();
        let mut n = 2;
        while taken.contains(&name) {
            name = format!("{base}-{n}");
            n += 1;
        }
        taken.push(name.clone());
        let mut entry = vec![format!("  {}:", quoted(&name))];
        for (_, chunk) in unit.body.iter().filter(|(k, _)| k != "alert") {
            entry.extend(chunk.iter().cloned());
        }
        entries.push(entry.join("\n"));
        names.insert(p, name);
    }
    (names, entries)
}
