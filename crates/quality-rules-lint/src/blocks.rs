//! A rules file's text in blocks, for moving them between files with
//! their comments (decision 0044, _Split by field_): its top-level
//! sections, the named entries of a section, the items of `rules:`. Only
//! block style is cut; a section written inline is said so.

/// A top-level section: its key, what follows the key on its line, and
/// its lines below (each comment just above it included in the section).
#[derive(Debug, Clone)]
pub(crate) struct Section {
    pub key: String,
    pub inline: String,
    pub body: Vec<String>,
}

/// One entry of a section, or one rule of `rules:`: its name (none for a
/// rule), and its lines, the comments just above it first, re-indented to
/// two spaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Block {
    pub name: String,
    pub lines: Vec<String>,
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

fn blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// The comment lines before the first section, and the sections.
pub(crate) fn sections(yaml: &str) -> (Vec<String>, Vec<Section>) {
    let lines: Vec<&str> = yaml.lines().collect();
    let mut preamble = Vec::new();
    let mut out: Vec<Section> = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    for line in lines {
        let top = indent(line) == 0 && !blank(line) && !is_comment(line);
        if top {
            let (key, rest) = line.split_once(':').unwrap_or((line, ""));
            let rest = rest.split(" #").next().unwrap_or("").trim().to_string();
            // Comments at the margin before the first key are the file's;
            // later, they close the section above.
            match out.last_mut() {
                Some(last) => last.body.append(&mut pending),
                None => preamble.append(&mut pending),
            }
            out.push(Section {
                key: key.trim().to_string(),
                inline: rest,
                body: Vec::new(),
            });
        } else if indent(line) == 0 && is_comment(line) {
            pending.push(line.to_string());
        } else {
            match out.last_mut() {
                Some(last) => {
                    last.body.append(&mut pending);
                    last.body.push(line.to_string());
                }
                None => preamble.push(line.to_string()),
            }
        }
    }
    if let Some(last) = out.last_mut() {
        last.body.append(&mut pending);
    } else {
        preamble.append(&mut pending);
    }
    (preamble, out)
}

/// A section's body cut at its blocks: lines at the first block's indent
/// that start one (`- ` for rules, `name:` for a map), each with the
/// comments just above it and what is indented under it.
pub(crate) fn blocks(body: &[String], items: bool) -> Vec<Block> {
    let Some(at) = body
        .iter()
        .find(|l| !blank(l) && !is_comment(l))
        .map(|l| indent(l))
    else {
        return Vec::new();
    };
    let starts = |l: &str| {
        indent(l) == at
            && !blank(l)
            && !is_comment(l)
            && (!items || l.trim_start().starts_with('-'))
    };
    let mut out: Vec<Block> = Vec::new();
    let mut comments: Vec<String> = Vec::new();
    for line in body {
        if starts(line) {
            let name = if items {
                String::new()
            } else {
                let key = line.trim_start().split(':').next().unwrap_or("");
                key.trim()
                    .trim_matches(|c| c == '"' || c == '\'')
                    .to_string()
            };
            let mut lines = std::mem::take(&mut comments);
            lines.push(line.clone());
            out.push(Block { name, lines });
        } else if is_comment(line) && indent(line) <= at {
            comments.push(line.clone());
        } else if blank(line) {
            comments.clear();
            if let Some(last) = out.last_mut() {
                last.lines.push(String::new());
            }
        } else if let Some(last) = out.last_mut() {
            last.lines.append(&mut comments);
            last.lines.push(line.clone());
        }
    }
    for block in &mut out {
        while block.lines.last().is_some_and(|l| blank(l)) {
            block.lines.pop();
        }
        block.lines = block
            .lines
            .iter()
            .map(|l| {
                if blank(l) {
                    String::new()
                } else {
                    let keep = indent(l).saturating_sub(at);
                    format!("  {}{}", " ".repeat(keep), l.trim_start())
                }
            })
            .collect();
    }
    out
}

/// The text of a block's lines, compared without its comments' spacing.
pub(crate) fn same(a: &Block, b: &Block) -> bool {
    let words = |b: &Block| -> Vec<String> {
        b.lines
            .iter()
            .filter(|l| !blank(l) && !is_comment(l))
            .map(|l| l.trim().to_string())
            .collect()
    };
    words(a) == words(b)
}
