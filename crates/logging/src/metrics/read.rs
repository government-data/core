//! Reads a metrics file back into its samples, refusing a line the text
//! format does not allow: what each service's tests use to check every name
//! and every label value of what it writes. Alloy's own reading is checked
//! by `tools/verify/metrics_app_cert.sh`.

/// One line of a metrics file: a series and its value.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub name: String,
    pub labels: Vec<(String, String)>,
    pub value: f64,
}

impl Sample {
    /// The value of the label `name`, if the series has it.
    pub fn label(&self, name: &str) -> Option<&str> {
        self.labels
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Every sample of `text`, or the first line that is not the text format.
pub fn samples(text: &str) -> Result<Vec<Sample>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let bad = |why: &str| format!("line {}: {why}: {line:?}", n + 1);
        if let Some(comment) = line.strip_prefix('#') {
            let known = ["HELP ", "TYPE ", "UNIT ", "EOF"];
            if !known.iter().any(|k| comment.trim_start().starts_with(k)) {
                return Err(bad("an unknown comment"));
            }
            continue;
        }
        if line.trim().is_empty() {
            return Err(bad("an empty line"));
        }
        out.push(sample(line).map_err(|why| bad(&why))?);
    }
    Ok(out)
}

fn sample(line: &str) -> Result<Sample, String> {
    let end = line.find(['{', ' ']).ok_or("no value")?;
    let name = &line[..end];
    if !is_name(name, true) {
        return Err("a bad metric name".into());
    }
    let mut rest = &line[end..];
    let mut labels = Vec::new();
    if let Some(inner) = rest.strip_prefix('{') {
        let (parsed, after) = label_set(inner)?;
        labels = parsed;
        rest = after;
    }
    let value = rest.strip_prefix(' ').ok_or("no space before the value")?;
    if value.contains(' ') {
        return Err("a timestamp or more than one value".into());
    }
    let value = match value {
        "+Inf" => f64::INFINITY,
        "-Inf" => f64::NEG_INFINITY,
        v => v.parse().map_err(|_| "a value that is not a number")?,
    };
    Ok(Sample {
        name: name.to_string(),
        labels,
        value,
    })
}

type Labels = Vec<(String, String)>;

/// `k="v",…}` and what follows the closing brace.
fn label_set(mut rest: &str) -> Result<(Labels, &str), String> {
    let mut labels = Vec::new();
    loop {
        if let Some(after) = rest.strip_prefix('}') {
            return Ok((labels, after));
        }
        let eq = rest.find('=').ok_or("a label without a value")?;
        let key = &rest[..eq];
        if !is_name(key, false) {
            return Err(format!("a bad label name {key:?}"));
        }
        let (value, after) = quoted(
            rest[eq + 1..]
                .strip_prefix('"')
                .ok_or("an unquoted value")?,
        )?;
        labels.push((key.to_string(), value));
        rest = after.strip_prefix(',').unwrap_or(after);
    }
}

/// A label value up to its closing quote, with `\\`, `\"` and `\n`.
fn quoted(text: &str) -> Result<(String, &str), String> {
    let mut value = String::new();
    let mut chars = text.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Ok((value, &text[i + 1..])),
            '\\' => match chars.next() {
                Some((_, '\\')) => value.push('\\'),
                Some((_, '"')) => value.push('"'),
                Some((_, 'n')) => value.push('\n'),
                _ => return Err("a bad escape".into()),
            },
            c => value.push(c),
        }
    }
    Err("an unclosed label value".into())
}

fn is_name(name: &str, colon: bool) -> bool {
    let ok = |c: char| c == '_' || c.is_ascii_alphabetic() || (colon && c == ':');
    let mut chars = name.chars();
    chars.next().is_some_and(ok) && chars.all(|c| ok(c) || c.is_ascii_digit())
}
