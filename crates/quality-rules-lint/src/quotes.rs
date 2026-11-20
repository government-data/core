//! Quotes a value does not need (decision 0041, `QR203`): a quoted value
//! whose quotes can go because, written plain, the YAML reads back the same
//! text. The test is the YAML itself, on the line alone when it reads alone
//! and on the whole file otherwise; besides, a value keeps its quotes when
//! another reader could take it for something else (`yes`, a date) or it
//! starts with a sign YAML reads (`:cut`).

use serde_json::Value;

use crate::style::at;
use crate::{Code, Finding};

/// A quoted value on a line: its bytes (quotes included) and its text.
pub(crate) struct Quoted {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) text: String,
}

/// The quoted values of `yaml` that read the same without their quotes.
pub(crate) fn findings(yaml: &str) -> Vec<Finding> {
    let lines: Vec<&str> = yaml.split_inclusive('\n').collect();
    needless(yaml)
        .into_iter()
        .map(|(i, q)| {
            let column = lines[i][..q.start].chars().count();
            Finding::new(Code::NeedlessQuotes, None, at(i, column))
        })
        .collect()
}

/// Each quoted value that reads the same without its quotes, by line.
pub(crate) fn needless(yaml: &str) -> Vec<(usize, Quoted)> {
    let Some(whole) = read(yaml) else {
        return Vec::new(); // what does not read is QR001's
    };
    let mut out = Vec::new();
    let mut offset = 0;
    let mut in_flow = false;
    for (i, line) in yaml.split_inclusive('\n').enumerate() {
        for q in quoted(line) {
            if plain_is_safe(&q.text) && reads_same(yaml, &whole, (line, offset, in_flow), &q) {
                out.push((i, q));
            }
        }
        offset += line.len();
        let kept = line.trim_end();
        if !kept.is_empty() && !kept.trim_start().starts_with('#') {
            in_flow = kept.ends_with(['[', '{', ',']);
        }
    }
    out
}

pub(crate) fn read(yaml: &str) -> Option<Value> {
    serde_saphyr::from_str::<Value>(yaml).ok()
}

/// The line alone (not inside a flow list or map of several lines), or as
/// the item of a flow list, when it reads so; else the whole file.
fn reads_same(yaml: &str, whole: &Value, place: (&str, usize, bool), q: &Quoted) -> bool {
    let (line, offset, in_flow) = place;
    let plain = format!("{}{}{}", &line[..q.start], q.text, &line[q.end..]);
    let alone = line.trim_start_matches(' ');
    let cut = line.len() - alone.len();
    let wrappers: &[(&str, &str)] = if in_flow {
        &[("[", "]")]
    } else {
        &[("", ""), ("[", "]")]
    };
    for &(open, close) in wrappers {
        let wrap = |s: &str| format!("{open}{}{close}", s.trim_end());
        if let Some(before) = read(&wrap(alone)) {
            return read(&wrap(&plain[cut..])).is_some_and(|after| after == before);
        }
    }
    let changed = format!("{}{plain}{}", &yaml[..offset], &yaml[offset + line.len()..]);
    read(&changed).is_some_and(|after| after == *whole)
}

/// Words other YAML readers (1.1) take for a boolean or nothing.
const KEYWORDS: [&str; 12] = [
    "y", "n", "yes", "no", "on", "off", "true", "false", "null", "~", "nan", "inf",
];

/// Whether a text could go plain at all, before asking the YAML.
fn plain_is_safe(text: &str) -> bool {
    let Some(first) = text.chars().next() else {
        return false;
    };
    let lower = text
        .trim_start_matches(['.', '+', '-'])
        .to_ascii_lowercase();
    !KEYWORDS.contains(&lower.as_str())
        && !"-?:,[]{}#&*!|>'\"%@`".contains(first)
        && !text.starts_with(char::is_whitespace)
        && !text.ends_with(char::is_whitespace)
        && !text.chars().any(char::is_control)
        && !looks_like_date(text)
}

fn looks_like_date(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() >= 10
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
}

/// What a line holds before a comment: its quoted values, where the
/// comment starts with the sign before it (`:` in `node: #1651`), and a
/// quote opened that the line does not close.
struct Scan {
    quoted: Vec<Quoted>,
    comment: Option<(usize, Option<u8>)>,
    unclosed: Option<usize>,
}

/// The quoted values a line starts (after `: `, `- `, `[`, `{`, `,`), up to
/// a comment; one that goes on to the next line is left alone.
fn scan(line: &str) -> Scan {
    let b = line.as_bytes();
    let mut quoted = Vec::new();
    let mut last = None::<u8>;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'#' && (i == 0 || b[i - 1] == b' ') {
            return Scan {
                quoted,
                comment: Some((i, last)),
                unclosed: None,
            };
        }
        let starts = matches!(last, None | Some(b':' | b'-' | b'[' | b'{' | b',' | b'?'));
        if (c == b'"' || c == b'\'') && starts {
            let Some((end, text)) = closing(line, i) else {
                return Scan {
                    quoted,
                    comment: None,
                    unclosed: Some(i),
                };
            };
            if let Some(text) = text {
                quoted.push(Quoted {
                    start: i,
                    end,
                    text,
                });
            }
            last = Some(c);
            i = end;
            continue;
        }
        if !c.is_ascii_whitespace() {
            last = Some(c);
        }
        i += 1;
    }
    Scan {
        quoted,
        comment: None,
        unclosed: None,
    }
}

/// Where a line's comment starts, and the sign before it.
pub(crate) fn comment(line: &str) -> Option<(usize, Option<u8>)> {
    scan(line).comment
}

fn quoted(line: &str) -> Vec<Quoted> {
    scan(line).quoted
}

/// The quote a line opens and does not close, if any.
pub(crate) fn opens(line: &str) -> Option<char> {
    scan(line).unclosed.map(|i| line.as_bytes()[i] as char)
}

/// A quote its line does not close (`QR207`): what follows, up to the next
/// quote lines later, is read into the value.
pub(crate) fn unclosed_quotes(yaml: &str) -> Vec<Finding> {
    yaml.lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let start = scan(line).unclosed?;
            let column = line[..start].chars().count();
            Some(Finding::new(Code::UnclosedQuote, None, at(i, column)))
        })
        .collect()
}

/// A value lost to a comment (`QR206`): a `#` right where a value starts
/// (`node: #1651`, `[a, #b]`), with text after it, reads as a comment.
pub(crate) fn lost_values(yaml: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    for (i, line) in yaml.lines().enumerate() {
        let Some((at_hash, before)) = scan(line).comment else {
            continue;
        };
        let rest = &line[at_hash + 1..];
        let value: String = rest
            .chars()
            .take_while(|c| !c.is_whitespace() && !matches!(c, ',' | '}' | ']'))
            .collect();
        let starts_value = matches!(before, Some(b':' | b'-' | b'[' | b'{' | b','));
        if starts_value && !value.is_empty() {
            let column = line[..at_hash].chars().count();
            let f = Finding::new(Code::LostValue, None, at(i, column));
            out.push(f.with("value", format!("#{value}")));
        }
    }
    out
}

/// Where the quoted value at `start` ends, and its text if it is plain
/// enough to decode here (escapes other than `\\`, `\"` and `\/` keep it).
pub(crate) fn closing(line: &str, start: usize) -> Option<(usize, Option<String>)> {
    let b = line.as_bytes();
    let quote = b[start];
    let mut text = String::new();
    let mut decodable = true;
    let mut i = start + 1;
    while i < b.len() {
        let c = b[i];
        if quote == b'"' && c == b'\\' {
            match b.get(i + 1) {
                Some(&e @ (b'\\' | b'"' | b'/')) => text.push(e as char),
                _ => decodable = false,
            }
            i += 2;
            continue;
        }
        if c == quote {
            if quote == b'\'' && b.get(i + 1) == Some(&b'\'') {
                text.push('\'');
                i += 2;
                continue;
            }
            return Some((i + 1, decodable.then_some(text)));
        }
        let ch = line[i..].chars().next()?;
        text.push(ch);
        i += ch.len_utf8();
    }
    None
}
