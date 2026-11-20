//! Lint of a rules file (decision 0019, point 8): in milliseconds and with
//! no data, what keeps a rule from compiling or makes it suspect, with its
//! place in the YAML. Errors block saving; warnings do not. Each finding
//! carries a code and the values of its message; the text is the caller's,
//! from `texts` (`quality.lint.<code>`), in Spanish and English.
//!
//! The rules' own examples (point 8, decision 0033 point 7) become one
//! query per rule, which the caller runs on an in-memory DuckDB and hands
//! back to [`check_examples`].

mod blocks;
mod checks;
mod continued;
mod examples;
mod fold;
mod globals;
mod patterns;
mod problems;
mod quotes;
mod rename;
mod split;
mod style;
mod suggest;
mod tidy;

use quality_rules_compile::{compile, Compiled, Schema};
use quality_rules_model::{parse, Position, RulesFile};

pub use examples::{check_examples, ExampleQuery, Expected};
pub use globals::Extended;
pub use patterns::split_and_extract;
pub use rename::{references, rename_fields, Renames};
pub use split::{split_by_field, Change, SplitRefused};
pub use tidy::tidy;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
}

/// The checks, each with its code (`QR001`…, `QR100`… for decision 0034,
/// `QR200`… for 0041).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Code {
    /// The YAML cannot be read.
    Syntax,
    /// An expression outside the grammar.
    Expression,
    UnknownField,
    UnknownName,
    NotApplicable,
    Missing,
    BadValue,
    NotSupported,
    DuplicateId,
    /// A regular expression with what RE2 lacks.
    Regex,
    /// An example that does not give the result it declares.
    Example,
    /// The same check as another rule.
    DuplicateRule,
    NoAlert,
    RequiredAndPlaceholder,
    NeverHolds,
    TextOnNumber,
    LargeFile,
    LargeDraft,
    // QR102, a long expression, is retired (decision 0052): never reused.
    LongList,
    /// Not two spaces a level (decision 0041).
    Indentation,
    Tab,
    TrailingSpace,
    /// Quotes a value does not need: it reads the same without them.
    NeedlessQuotes,
    LongLine,
    /// Rules not separated by exactly one blank line.
    RuleSpacing,
    /// A `#` where a value starts, which YAML reads as a comment.
    LostValue,
    /// A quote its line does not close.
    UnclosedQuote,
    /// A line that goes on the value above, at its key's indent.
    Continued,
    /// A block value (`>-`, `|`) on a key whose value is a map of names.
    BlockOnMap,
    /// A rule in a file that is not its field's own (decision 0044).
    OtherFieldsFile,
    /// An `extends` that names no named rule (decision 0045).
    UnknownNamed,
    /// A name declared both by the file and by the table's global rules.
    DeclaredTwice,
    /// The table's global rules with rules of their own.
    RulesInGlobals,
    /// A key a rule writes with the value of the named rule it extends.
    RepeatsNamed,
}

impl Code {
    pub fn id(self) -> &'static str {
        match self {
            Self::Syntax => "QR001",
            Self::Expression => "QR002",
            Self::UnknownField => "QR003",
            Self::UnknownName => "QR004",
            Self::NotApplicable => "QR005",
            Self::Missing => "QR006",
            Self::BadValue => "QR007",
            Self::NotSupported => "QR008",
            Self::DuplicateId => "QR009",
            Self::Regex => "QR010",
            Self::Example => "QR011",
            Self::UnknownNamed => "QR012",
            Self::DeclaredTwice => "QR013",
            Self::RulesInGlobals => "QR014",
            Self::DuplicateRule => "QR020",
            Self::NoAlert => "QR021",
            Self::RequiredAndPlaceholder => "QR022",
            Self::NeverHolds => "QR023",
            Self::TextOnNumber => "QR024",
            Self::LargeFile => "QR100",
            Self::LargeDraft => "QR101",
            Self::LongList => "QR103",
            Self::Indentation => "QR200",
            Self::Tab => "QR201",
            Self::TrailingSpace => "QR202",
            Self::NeedlessQuotes => "QR203",
            Self::LongLine => "QR204",
            Self::RuleSpacing => "QR205",
            Self::LostValue => "QR206",
            Self::UnclosedQuote => "QR207",
            Self::Continued => "QR208",
            Self::BlockOnMap => "QR209",
            Self::OtherFieldsFile => "QR210",
            Self::RepeatsNamed => "QR211",
        }
    }

    /// The key of its message in `texts`.
    pub fn key(self) -> String {
        format!("quality.lint.{}", self.id().to_ascii_lowercase())
    }

    pub fn severity(self) -> Severity {
        match self {
            Self::DuplicateRule
            | Self::NoAlert
            | Self::RequiredAndPlaceholder
            | Self::NeverHolds
            | Self::TextOnNumber
            | Self::LargeFile
            | Self::LargeDraft
            | Self::LongList
            | Self::Indentation
            | Self::Tab
            | Self::TrailingSpace
            | Self::NeedlessQuotes
            | Self::LongLine
            | Self::RuleSpacing
            | Self::LostValue
            | Self::UnclosedQuote
            | Self::Continued
            | Self::BlockOnMap
            | Self::OtherFieldsFile
            | Self::RepeatsNamed => Severity::Warning,
            _ => Severity::Error,
        }
    }

    pub const ALL: [Self; 34] = [
        Self::Syntax,
        Self::Expression,
        Self::UnknownField,
        Self::UnknownName,
        Self::NotApplicable,
        Self::Missing,
        Self::BadValue,
        Self::NotSupported,
        Self::DuplicateId,
        Self::Regex,
        Self::Example,
        Self::DuplicateRule,
        Self::NoAlert,
        Self::RequiredAndPlaceholder,
        Self::NeverHolds,
        Self::TextOnNumber,
        Self::LargeFile,
        Self::LargeDraft,
        Self::LongList,
        Self::Indentation,
        Self::Tab,
        Self::TrailingSpace,
        Self::NeedlessQuotes,
        Self::LongLine,
        Self::RuleSpacing,
        Self::LostValue,
        Self::UnclosedQuote,
        Self::Continued,
        Self::BlockOnMap,
        Self::OtherFieldsFile,
        Self::UnknownNamed,
        Self::DeclaredTwice,
        Self::RulesInGlobals,
        Self::RepeatsNamed,
    ];
}

/// One finding: what, where, and the values its message names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub code: Code,
    /// The rule's id as written, if it is about one rule.
    pub rule: Option<String>,
    pub at: Option<Position>,
    pub values: Vec<(&'static str, String)>,
}

impl Finding {
    pub fn severity(&self) -> Severity {
        self.code.severity()
    }

    fn new(code: Code, rule: Option<&str>, at: Option<Position>) -> Self {
        Self {
            code,
            rule: rule.map(str::to_string),
            at,
            values: Vec::new(),
        }
    }

    fn with(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.values.push((name, value.into()));
        self
    }
}

/// What lint found, the rules that compiled, their examples to run, and
/// the rules that extend a named one (decision 0045).
#[derive(Debug, Clone)]
pub struct Report {
    pub findings: Vec<Finding>,
    pub compiled: Vec<Compiled>,
    pub examples: Vec<ExampleQuery>,
    pub extends: Vec<Extended>,
}

impl Report {
    /// Errors block saving; warnings do not.
    pub fn blocks(&self) -> bool {
        self.findings
            .iter()
            .any(|f| f.severity() == Severity::Error)
    }
}

/// Lints a rules file against its dataset's fields.
pub fn lint(yaml: &str, schema: &Schema) -> Report {
    lint_with(yaml, schema, None)
}

/// Lints the table's global rules themselves: their declarations, and
/// that they have no rules of their own (decision 0045).
pub fn lint_globals(yaml: &str, schema: &Schema) -> Report {
    let mut report = lint(yaml, schema);
    if let Some(finding) = parse(yaml)
        .ok()
        .as_ref()
        .and_then(globals::rules_in_globals)
    {
        report.findings.insert(0, finding);
    }
    report
}

/// Lints a rules file against its dataset's fields, with the table's
/// global rules (their YAML) when there are (decision 0045).
pub fn lint_with(yaml: &str, schema: &Schema, global_rules: Option<&str>) -> Report {
    let file = match parse(yaml) {
        Ok(file) => file,
        Err(e) => {
            let mut style = style::findings(yaml);
            let finding = match cause_before(&mut style, e.at) {
                // The reader stops later than the cause (a value cut off by
                // `#`, a quote left open): say so, there.
                Some((at, key)) => Finding::new(Code::Syntax, None, at).with("message_key", key),
                None => Finding::new(Code::Syntax, None, e.at).with("message", e.message),
            };
            let mut findings = vec![finding];
            findings.extend(style);
            return Report {
                findings,
                compiled: Vec::new(),
                examples: Vec::new(),
                extends: Vec::new(),
            };
        }
    };
    let (file, merged, extends) = globals::merge(file, global_rules);
    let (compiled, problems) = compile(&file, schema);
    let mut findings = problems::findings(&file, schema, problems);
    findings.extend(merged);
    findings.extend(checks::file(&file, yaml, schema, &compiled));
    let (queries, example_findings) = examples::queries(&file, schema, &compiled);
    findings.extend(example_findings);
    findings.extend(style::findings(yaml));
    findings.sort_by_key(|f| (f.at, f.code));
    // The same finding at the same place once (a field named twice in one
    // expression is one finding).
    findings.dedup();
    Report {
        findings,
        compiled,
        examples: queries,
        extends,
    }
}

/// The first value lost to a comment (`QR206`), to a quote left open
/// (`QR207`), continued at its key's indent (`QR208`) or a map made one
/// text by a block header (`QR209`) at or before where the YAML stopped reading, taken out of
/// `style`: the likely cause of the error, with the key of its reason.
fn cause_before(
    style: &mut Vec<Finding>,
    stopped: Option<Position>,
) -> Option<(Option<Position>, &'static str)> {
    let i = style.iter().position(|f| {
        matches!(
            f.code,
            Code::LostValue | Code::UnclosedQuote | Code::Continued | Code::BlockOnMap
        ) && stopped.is_none_or(|s| f.at.is_some_and(|at| at <= s))
    })?;
    let cause = style.remove(i);
    let key = match cause.code {
        Code::LostValue => "quality.lint.syntax.lost_value",
        Code::Continued => "quality.lint.syntax.continued",
        Code::BlockOnMap => "quality.lint.syntax.block_on_map",
        _ => "quality.lint.syntax.unclosed_quote",
    };
    Some((cause.at, key))
}

/// The rules of `yaml`, linted as `report`, that are not in their field's
/// own file `file` (decision 0044): a rule's field is its `field` (the
/// first, for a list), or else the first field it reads.
pub fn placement(file: &str, yaml: &str, report: &Report) -> Vec<Finding> {
    let (Some(stem), Ok(rules)) = (file.strip_suffix(".yaml"), parse(yaml)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for rule in &rules.rules {
        let id = &rule.id.value;
        let Some(compiled) = report
            .compiled
            .iter()
            .find(|c| c.id == *id || c.id.starts_with(&format!("{id}.")))
        else {
            continue;
        };
        let field = compiled
            .field
            .clone()
            .or_else(|| compiled.reads().into_iter().next());
        if let Some(field) = field.filter(|f| f != stem) {
            out.push(
                Finding::new(Code::OtherFieldsFile, Some(id), Some(rule.id.at))
                    .with("file", format!("{field}.yaml"))
                    .with("field", field),
            );
        }
    }
    out
}

/// A draft with more changed rules than one person can review (0034).
pub fn draft(changed_rules: usize) -> Option<Finding> {
    (changed_rules > 25).then(|| {
        Finding::new(Code::LargeDraft, None, None).with("count", changed_rules.to_string())
    })
}

fn rule_of<'a>(file: &'a RulesFile, id: &str) -> Option<&'a quality_rules_model::Rule> {
    file.rules
        .iter()
        .find(|r| r.id.value == id || id.starts_with(&format!("{}.", r.id.value)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use texts::Language;

    /// Every key a finding can name has its text in Spanish and English.
    #[test]
    fn every_message_has_its_text() {
        let mut keys: Vec<String> = Code::ALL.iter().map(|c| c.key()).collect();
        keys.push("quality.lint.suggestion".into());
        keys.extend(
            problems::REASONS
                .iter()
                .map(|r| format!("quality.lint.reason.{r}")),
        );
        for k in [
            "population",
            "list",
            "table",
            "reference",
            "derived",
            "named",
        ] {
            keys.push(format!("quality.lint.kind.{k}"));
        }
        keys.push("quality.lint.unsupported.table_on_several_fields".into());
        keys.push("quality.lint.syntax.lost_value".into());
        keys.push("quality.lint.syntax.unclosed_quote".into());
        keys.push("quality.lint.syntax.continued".into());
        keys.push("quality.lint.syntax.block_on_map".into());
        for k in ["fails", "passes", "skips"] {
            keys.push(format!("quality.lint.example.{k}"));
        }
        for language in [Language::Es, Language::En] {
            let catalog = texts::keys(language);
            let missing: Vec<&String> = keys.iter().filter(|k| !catalog.contains(k)).collect();
            assert!(missing.is_empty(), "{language:?}: {missing:?}");
        }
    }
}
