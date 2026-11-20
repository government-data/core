//! Tidy (decision 0041, point 3): a rules file rewritten without what its
//! style checks find that is mechanical. Tabs and trailing spaces go, a
//! value lost to a comment is quoted, needless quotes go, rules get one
//! blank line between them, and a long value is folded. Each step is kept
//! only if the file still reads the same; a file that does not read, even
//! with its lost values given back, is left as it is.

use crate::fold::{fold, WIDTH};
use crate::quotes::{self, closing, read};

/// Keys whose value is an expression; and the sections whose entries are.
const EXPRESSIONS: [&str; 5] = ["fails", "if", "then", "when", "where"];
const EXPRESSION_SECTIONS: [&str; 2] = ["derived", "populations"];

/// `yaml` tidied, reading the same but for the values lost to a comment.
pub fn tidy(yaml: &str) -> String {
    // What the file means but does not say comes back first: a value lost
    // to a comment (QR206), a value continued at its key's indent (QR208);
    // they may be what keeps it from reading at all.
    let given_back = lost_values(&crate::continued::repair(yaml));
    let (start, before) = match (read(&given_back), read(yaml)) {
        (Some(v), _) => (given_back, v),
        (None, Some(v)) => (yaml.to_string(), v),
        (None, None) => return yaml.to_string(),
    };
    let steps: [fn(&str) -> String; 4] = [whitespace, unquote, spacing, fold_long];
    steps.iter().fold(start, |text, step| {
        let next = step(&text);
        if read(&next).as_ref() == Some(&before) {
            next
        } else {
            text
        }
    })
}

/// `lines` joined back, with the final newline `yaml` had.
fn join(lines: &[String], yaml: &str) -> String {
    let mut out = lines.join("\n");
    if yaml.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Tabs in the indentation as two spaces, then no spaces at the end of a
/// line, outside a quoted value of several lines.
fn whitespace(yaml: &str) -> String {
    let mut open: Option<char> = None;
    let lines: Vec<String> = yaml
        .lines()
        .map(|line| {
            if let Some(q) = open {
                if line.contains(q) {
                    open = None;
                }
                return line.to_string();
            }
            open = quotes::opens(line);
            let body = line.trim_start_matches([' ', '\t']);
            let lead = &line[..line.len() - body.len()];
            // A line of only spaces is empty.
            let lead = if body.trim().is_empty() {
                String::new()
            } else {
                lead.replace('\t', "  ")
            };
            let body = if open.is_some() {
                body
            } else {
                body.trim_end()
            };
            format!("{lead}{body}")
        })
        .collect();
    join(&lines, yaml)
}

/// `node: #1651` quoted: `node: "#1651"`.
fn lost_values(yaml: &str) -> String {
    let lines: Vec<String> = yaml
        .lines()
        .map(|line| {
            let Some((hash, Some(b':' | b'-' | b'[' | b'{' | b','))) = quotes::comment(line) else {
                return line.to_string();
            };
            let rest = &line[hash..];
            let len = rest
                .find(|c: char| c.is_whitespace() || matches!(c, ',' | '}' | ']'))
                .unwrap_or(rest.len());
            if len < 2 || rest[..len].contains('"') {
                return line.to_string();
            }
            format!("{}\"{}\"{}", &line[..hash], &rest[..len], &rest[len..])
        })
        .collect();
    join(&lines, yaml)
}

/// The quotes a value does not need, gone.
fn unquote(yaml: &str) -> String {
    let mut lines: Vec<String> = yaml.lines().map(str::to_string).collect();
    let mut found = quotes::needless(yaml);
    // From the end of each line, so the earlier places stay where they are.
    found.sort_by(|(a, x), (b, y)| (b, y.start).cmp(&(a, x.start)));
    for (i, q) in found {
        if let Some(line) = lines.get_mut(i) {
            line.replace_range(q.start..q.end, &q.text);
        }
    }
    join(&lines, yaml)
}

/// One blank line before each rule but the first (a comment just above a
/// rule goes with it).
fn spacing(yaml: &str) -> String {
    let lines: Vec<&str> = yaml.lines().collect();
    let Some(start) = lines.iter().position(|l| l.starts_with("rules:")) else {
        return yaml.to_string();
    };
    let indent = |l: &str| l.len() - l.trim_start().len();
    let mut out: Vec<String> = lines[..=start].iter().map(|l| l.to_string()).collect();
    let mut item: Option<usize> = None;
    let mut first = true;
    let mut led = false;
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        let trimmed = line.trim_start();
        let ended = !line.is_empty() && indent(line) == 0 && !trimmed.starts_with(['#', '-']);
        let is_item = !ended && (trimmed.starts_with("- ") || trimmed == "-");
        let opens = is_item && *item.get_or_insert(indent(line)) == indent(line);
        let comment_opens = !ended
            && trimmed.starts_with('#')
            && item == Some(indent(line))
            && next_is_item(&lines[i + 1..], indent(line));
        // A comment just above a rule takes the blank line; the rule then
        // follows it directly.
        let leads = comment_opens || (opens && !std::mem::take(&mut led));
        if comment_opens {
            led = true;
        }
        if leads && !std::mem::take(&mut first) {
            while out.last().is_some_and(|l| l.trim().is_empty()) {
                out.pop();
            }
            out.push(String::new());
        }
        out.push(line.to_string());
        if ended {
            out.extend(lines[i + 1..].iter().map(|l| l.to_string()));
            break;
        }
    }
    join(&out, yaml)
}

/// Whether the comments from here lead straight to a rule at `indent`.
fn next_is_item(rest: &[&str], indent: usize) -> bool {
    rest.iter()
        .find(|l| !l.trim_start().starts_with('#'))
        .is_some_and(|l| {
            l.trim_start().starts_with("- ") && l.len() - l.trim_start().len() == indent
        })
}

/// A value of one line over the width, of a block mapping's key, folded.
fn fold_long(yaml: &str) -> String {
    let mut out = Vec::new();
    let mut section = String::new();
    for line in yaml.lines() {
        if line.starts_with(|c: char| c.is_ascii_alphabetic()) {
            section = line.split(':').next().unwrap_or_default().to_string();
        }
        match folded(line, &section) {
            Some(lines) => out.extend(lines),
            None => out.push(line.to_string()),
        }
    }
    join(&out, yaml)
}

/// `line` as a key and its value folded in `>-`, if it is long and can be.
fn folded(line: &str, section: &str) -> Option<Vec<String>> {
    if line.chars().count() <= WIDTH {
        return None;
    }
    let spaces = line.len() - line.trim_start().len();
    let body = &line[spaces..];
    let dash = if body.starts_with("- ") { 2 } else { 0 };
    let (key, value) = body[dash..].split_once(": ")?;
    let key_ok = !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    let value = value.trim_end();
    let text = match value.as_bytes().first()? {
        b'"' | b'\'' => match closing(value, 0)? {
            (end, Some(text)) if end == value.len() => text,
            _ => return None,
        },
        b'[' | b'{' | b'>' | b'|' | b'&' | b'*' | b'!' | b'#' => return None,
        _ if value.contains(" #") => return None,
        _ => value.to_string(),
    };
    if !key_ok {
        return None;
    }
    let entry = spaces == 2 && dash == 0 && EXPRESSION_SECTIONS.contains(&section);
    let expression = entry || (EXPRESSIONS.contains(&key) && section != "tables");
    let inner = spaces + dash + 2;
    let lines = fold(&text, WIDTH - inner, expression)?;
    let mut out = vec![format!("{}{key}: >-", &line[..spaces + dash])];
    out.extend(
        lines
            .into_iter()
            .map(|l| format!("{}{l}", " ".repeat(inner))),
    );
    Some(out)
}
