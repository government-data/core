//! A long value folded (decision 0041, point 3): the lines of a `>-` block
//! that joins them back, with one space each, into the same text. An
//! expression breaks before each clause, then at spaces where a line is
//! still long; a text at spaces. Only a single space outside a quoted text
//! is a break, so the joined lines are the value, character for character.

/// The longest line.
pub(crate) const WIDTH: usize = 120;

/// Words before which an expression starts a line.
const CLAUSES: [&str; 7] = ["CASE", "WHEN", "THEN", "ELSE", "END", "AND", "OR"];

/// The words of `value` between its breaks: single spaces outside quotes.
fn words(value: &str) -> Vec<&str> {
    let b = value.as_bytes();
    let mut out = Vec::new();
    let mut quoted = false;
    let mut start = 0;
    for (i, &c) in b.iter().enumerate() {
        if c == b'\'' {
            quoted = !quoted;
        }
        let single = c == b' '
            && !quoted
            && i > 0
            && i + 1 < b.len()
            && b[i - 1] != b' '
            && b[i + 1] != b' ';
        if single {
            out.push(&value[start..i]);
            start = i + 1;
        }
    }
    out.push(&value[start..]);
    out
}

/// `value` in lines of at most `width` characters (a word longer stays
/// whole); an expression breaks before its clauses first. `None` when it
/// fits on one line or cannot be folded back into the same text.
pub(crate) fn fold(value: &str, width: usize, expression: bool) -> Option<Vec<String>> {
    let mut lines: Vec<Vec<&str>> = vec![Vec::new()];
    let mut between = false;
    for word in words(value) {
        let upper = word.to_ascii_uppercase();
        let clause =
            expression && CLAUSES.contains(&upper.as_str()) && !(upper == "AND" && between);
        if upper == "BETWEEN" {
            between = true;
        } else if upper == "AND" {
            between = false;
        }
        let line = lines.last_mut().expect("one line at least");
        let length: usize = line.iter().map(|w| w.chars().count() + 1).sum();
        if !line.is_empty() && (clause || length + word.chars().count() > width) {
            lines.push(vec![word]);
        } else {
            line.push(word);
        }
    }
    let lines: Vec<String> = lines.into_iter().map(|l| l.join(" ")).collect();
    let same = lines.join(" ") == value;
    (lines.len() > 1 && same && lines.iter().all(|l| !l.is_empty())).then_some(lines)
}
