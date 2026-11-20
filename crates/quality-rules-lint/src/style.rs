//! How a rules file is written (decision 0041): warnings on the text, line
//! by line, whether or not the YAML reads. Indentation (`QR200`), tabs
//! (`QR201`), trailing spaces (`QR202`), long lines (`QR204`) and the blank
//! line between rules (`QR205`); quotes a value does not need (`QR203`) and
//! a value lost to a comment (`QR206`) or to a quote left open (`QR207`)
//! are in [`crate::quotes`].

use quality_rules_model::Position;

use crate::{continued, quotes, Code, Finding};

/// The longest line, in characters.
const LONG_LINE: usize = 120;

/// The style warnings of `yaml`.
pub(crate) fn findings(yaml: &str) -> Vec<Finding> {
    let lines: Vec<&str> = yaml.lines().collect();
    let mut out = Vec::new();
    whitespace(&lines, &mut out);
    indentation(&lines, &mut out);
    separation(&lines, &mut out);
    out.extend(quotes::findings(yaml));
    out.extend(quotes::lost_values(yaml));
    out.extend(quotes::unclosed_quotes(yaml));
    out.extend(continued::findings(yaml));
    out.extend(continued::blocks_on_maps(yaml));
    out
}

/// A place from a 0-based line and character.
pub(crate) fn at(line: usize, column: usize) -> Option<Position> {
    Some(Position {
        line: line as u64 + 1,
        column: column as u64 + 1,
    })
}

fn whitespace(lines: &[&str], out: &mut Vec<Finding>) {
    for (i, line) in lines.iter().enumerate() {
        if let Some(tab) = line.chars().position(|c| c == '\t') {
            out.push(Finding::new(Code::Tab, None, at(i, tab)));
        }
        let kept = line.trim_end_matches([' ', '\t']);
        if kept.len() < line.len() {
            out.push(Finding::new(
                Code::TrailingSpace,
                None,
                at(i, kept.chars().count()),
            ));
        }
        let length = line.chars().count();
        if length > LONG_LINE {
            let f = Finding::new(Code::LongLine, None, at(i, LONG_LINE));
            out.push(f.with("length", length.to_string()));
        }
    }
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Where a line's content starts, past its list dashes (`  - id:` → 4).
fn content(line: &str) -> usize {
    let mut at = indent(line);
    while line[at..].starts_with("- ") {
        at += 2 + indent(&line[at + 2..]);
    }
    at
}

/// Whether a line opens a block scalar (`fails: >-`), whose text follows.
fn opens_block(line: &str) -> bool {
    let last = line.trim_end().rsplit(' ').next().unwrap_or_default();
    matches!(last, "|" | "|-" | "|+" | ">" | ">-" | ">+")
}

/// Two spaces a level: an even indentation that grows by two, and only
/// under a line that opens a block (`key:`) or a flow (`[`, `{`).
fn indentation(lines: &[&str], out: &mut Vec<Finding>) {
    let mut allowed = 0;
    let mut block: Option<usize> = None;
    let mut quote: Option<char> = None;
    for (i, line) in lines.iter().enumerate() {
        // Inside a quoted value of several lines, up to its closing quote.
        if let Some(q) = quote {
            if line.contains(q) {
                quote = None;
            }
            continue;
        }
        quote = quotes::opens(line);
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let here = indent(line);
        if let Some(b) = block {
            if here > b {
                continue;
            }
            block = None;
        }
        if here % 2 == 1 || here > allowed {
            out.push(Finding::new(Code::Indentation, None, at(i, 0)));
        }
        // Deeper only under a line that opens a block or a flow.
        let opens = line.trim_end().ends_with([':', '[', '{', ',']);
        allowed = content(line) + if opens { 2 } else { 0 };
        if opens_block(line) {
            block = Some(here);
        }
    }
}

/// Exactly one blank line before each rule but the first (a comment just
/// above a rule goes with it).
fn separation(lines: &[&str], out: &mut Vec<Finding>) {
    let Some(start) = lines.iter().position(|l| l.starts_with("rules:")) else {
        return;
    };
    let mut item: Option<usize> = None;
    let mut first = true;
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        let trimmed = line.trim_start();
        if !line.is_empty() && indent(line) == 0 && !trimmed.starts_with(['#', '-']) {
            break; // the next section
        }
        let is_item = trimmed.starts_with("- ") || trimmed == "-";
        if !is_item || *item.get_or_insert(indent(line)) != indent(line) {
            continue;
        }
        if !std::mem::take(&mut first) && blanks_above(lines, i, start) != 1 {
            out.push(Finding::new(Code::RuleSpacing, None, at(i, indent(line))));
        }
    }
}

fn blanks_above(lines: &[&str], mut i: usize, start: usize) -> usize {
    while i > start + 1 && lines[i - 1].trim_start().starts_with('#') {
        i -= 1;
    }
    let mut blanks = 0;
    while i > start + 1 && lines[i - 1].trim().is_empty() {
        blanks += 1;
        i -= 1;
    }
    blanks
}
