//! A value continued at its key's indent (decision 0041, `QR208`), and a
//! map's key given a block value (`derived: >-`, `QR209`). For the first, the
//! lines under `TotalIngresos: CASE WHEN …` that go on with it, indented
//! like the key, read as keys of their own and the YAML stops. Lint says
//! so at the first of them, and tidy puts the value and its lines in a
//! folded block (`>-`), which joins them with a space each, as meant. For
//! the second, the map becomes one text: tidy takes the header away, and
//! its names, each with its lines, are then the first case.

use crate::style::at;
use crate::{quotes, Code, Finding};

/// A key's line and the lines that go on its value.
struct Run {
    key: usize,
    lines: Vec<usize>,
}

fn spaces(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Whether a line is a key (`name:` then a space or its end).
fn is_key(line: &str) -> bool {
    let body = line.trim_start().trim_start_matches("- ");
    body.split_once(':').is_some_and(|(key, rest)| {
        !key.is_empty()
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            && (rest.is_empty() || rest.starts_with(char::is_whitespace))
    })
}

/// Where a key's content starts and its plain value, if `line` is a key
/// with a plain value on it (not quoted, a flow, a block or an anchor).
fn plain_pair(line: &str) -> Option<(usize, usize, &str)> {
    let lead = spaces(line);
    let dash = if line[lead..].starts_with("- ") { 2 } else { 0 };
    let body = &line[lead + dash..];
    let (key, value) = body.split_once(": ")?;
    let value = value.trim();
    let plain = !value.is_empty()
        && !value.starts_with(['"', '\'', '[', '{', '>', '|', '&', '*', '!', '#'])
        && !value.contains(" #");
    let key_ok = key
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    (plain && key_ok && !key.is_empty()).then_some((lead + dash, lead + dash + key.len(), value))
}

/// The runs of `lines`: outside block values and quoted values of several
/// lines, a key with a plain value followed by lines at its indent that
/// are neither keys, items, comments nor blank.
fn runs(lines: &[&str]) -> Vec<Run> {
    let mut out = Vec::new();
    let mut block: Option<usize> = None;
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(q) = quote {
            quote = (!line.contains(q)).then_some(q);
            i += 1;
            continue;
        }
        if block.is_some_and(|b| line.trim().is_empty() || spaces(line) > b) {
            i += 1;
            continue;
        }
        block = None;
        quote = quotes::opens(line);
        if line.trim_end().ends_with(['>', '|', '-', '+']) && line.contains(": ") {
            let last = line.trim_end().rsplit(' ').next().unwrap_or_default();
            if matches!(last, "|" | "|-" | "|+" | ">" | ">-" | ">+") {
                block = Some(spaces(line));
            }
        }
        let Some((indent, _, _)) = plain_pair(line) else {
            i += 1;
            continue;
        };
        // Its lines, at its indent or deeper; it is a run only if one is at
        // its indent (deeper only, the YAML reads them as the value).
        let goes_on = |l: &&str| {
            let t = l.trim_start();
            !t.is_empty() && !t.starts_with(['#', '-']) && spaces(l) >= indent && !is_key(l)
        };
        let next: Vec<usize> = (i + 1..lines.len())
            .take_while(|&j| goes_on(&lines[j]))
            .collect();
        i = next.last().map_or(i + 1, |last| last + 1);
        if next.iter().any(|&j| spaces(lines[j]) == indent) {
            out.push(Run {
                key: i - next.len() - 1,
                lines: next,
            });
        }
    }
    out
}

/// Keys whose value is a map of names: at the top of the file, and of a rule.
const TOP_MAPS: [&str; 6] = [
    "derived",
    "lists",
    "populations",
    "references",
    "tables",
    "placeholders",
];
const RULE_MAPS: [&str; 3] = ["examples", "origin", "for_each"];

/// A map's key given a block value (`derived: >-`): where its header is.
fn block_on_map(line: &str) -> Option<usize> {
    let lead = spaces(line);
    let body = line[lead..].trim_start_matches("- ");
    let (key, value) = body.split_once(':')?;
    let header = value.trim();
    let map = if lead == 0 {
        TOP_MAPS.contains(&key)
    } else {
        RULE_MAPS.contains(&key)
    };
    let block = matches!(header, "|" | "|-" | "|+" | ">" | ">-" | ">+");
    (map && block).then(|| line.len() - value.trim_start().len())
}

/// `QR209` at a block header on a map's key.
pub(crate) fn blocks_on_maps(yaml: &str) -> Vec<Finding> {
    yaml.lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let header = block_on_map(line)?;
            let column = line[..header].chars().count();
            let key = line
                .trim_start()
                .trim_start_matches("- ")
                .split(':')
                .next()?;
            let f = Finding::new(Code::BlockOnMap, None, at(i, column));
            Some(f.with("key", key).with("block", line[header..].trim_end()))
        })
        .collect()
}

/// A map's key without its block header: its lines are its names again.
fn unblock(yaml: &str) -> String {
    let lines: Vec<String> = yaml
        .lines()
        .map(|line| match block_on_map(line) {
            Some(header) => line[..header].trim_end().to_string(),
            None => line.to_string(),
        })
        .collect();
    let mut text = lines.join("\n");
    if yaml.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// `QR208` at the first line that goes on a value at its key's indent.
pub(crate) fn findings(yaml: &str) -> Vec<Finding> {
    let lines: Vec<&str> = yaml.lines().collect();
    runs(&lines)
        .into_iter()
        .map(|r| {
            let (indent, _, _) = plain_pair(lines[r.key]).expect("a run starts at a key");
            let first = *r
                .lines
                .iter()
                .find(|&&j| spaces(lines[j]) == indent)
                .expect("one at its indent");
            Finding::new(Code::Continued, None, at(first, spaces(lines[first])))
        })
        .collect()
}

/// Each run in a folded block under its key: `key: >-`, then the value and
/// the lines that go on it, two spaces deeper, without trailing spaces.
pub(crate) fn repair(yaml: &str) -> String {
    let unblocked = unblock(yaml);
    let yaml = unblocked.as_str();
    let lines: Vec<&str> = yaml.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut next = 0;
    for run in runs(&lines) {
        out.extend(lines[next..run.key].iter().map(|l| l.to_string()));
        let line = lines[run.key];
        let (indent, key_end, value) = plain_pair(line).expect("a run starts at a key");
        let deeper = " ".repeat(indent + 2);
        out.push(format!("{}: >-", &line[..key_end]));
        out.push(format!("{deeper}{value}"));
        out.extend(
            run.lines
                .iter()
                .map(|&j| format!("{deeper}{}", lines[j].trim())),
        );
        next = run.lines.last().expect("a run has lines") + 1;
    }
    out.extend(lines[next..].iter().map(|l| l.to_string()));
    let mut text = out.join("\n");
    if yaml.ends_with('\n') {
        text.push('\n');
    }
    text
}
