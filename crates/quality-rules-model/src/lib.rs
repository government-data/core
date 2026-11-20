//! The rules' YAML of Quality Data (decisions 0019, 0033): what a dataset
//! declares once and its rules, read with the place of every value. The
//! first stage of the rule language; `quality-rules-tree` and
//! `quality-rules-compile` build on it, and `quality-rules` gathers them.
//! Pure: it reads text and returns values.

mod at;
mod globals;
mod model;
mod rule;
mod shapes;

pub use at::{At, Position};
pub use globals::{repeats, with_globals, Unmerged, GLOBAL_FILE};
pub use model::*;

/// The YAML could not be read: where and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// `None` when the reader could not place it.
    pub at: Option<Position>,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.at {
            Some(at) => write!(f, "{}:{}: {}", at.line, at.column, self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for ParseError {}

/// Reads a rules file. Duplicate keys, unknown keys and values of the wrong
/// type are errors with their position; aliases are bounded, so a file
/// cannot expand without limit. A rule needs an id and a pattern, or a
/// named rule to take it from (decision 0045).
pub fn parse(yaml: &str) -> Result<RulesFile, ParseError> {
    let file: RulesFile = serde_saphyr::from_str(yaml).map_err(|e| ParseError {
        at: e.location().map(|l| Position {
            line: l.line(),
            column: l.column(),
        }),
        message: first_line(&e.to_string()),
    })?;
    globals::check_shape(&file)?;
    Ok(file)
}

/// The JSON Schema of a rules file, for editors (completion and errors).
pub fn json_schema() -> schemars::Schema {
    let mut schema = schemars::schema_for!(RulesFile);
    schema.insert("title".into(), "Quality Data rules".into());
    schema
}

/// The reader's message without its source snippet: the caller shows its own.
fn first_line(message: &str) -> String {
    let line = message.lines().next().unwrap_or_default();
    // "error: line 6 column 10: invalid floating point" → the reason only.
    line.trim_start_matches("error: ")
        .split_once(": ")
        .filter(|(place, _)| place.starts_with("line "))
        .map_or(line, |(_, reason)| reason)
        .to_string()
}
