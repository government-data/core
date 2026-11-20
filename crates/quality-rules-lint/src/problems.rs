//! What kept a rule from compiling, as findings, with a suggestion for a
//! name written almost right.

use quality_rules_compile::{Problem, ProblemKind, Schema};
use quality_rules_model::RulesFile;
use quality_rules_tree::Reason;

use super::suggest::closest;
use super::{Code, Finding};

pub(crate) fn findings(file: &RulesFile, schema: &Schema, problems: Vec<Problem>) -> Vec<Finding> {
    problems
        .into_iter()
        .map(|p| finding(file, schema, p))
        .collect()
}

fn finding(file: &RulesFile, schema: &Schema, p: Problem) -> Finding {
    let base = |code| Finding::new(code, Some(&p.rule), Some(p.at));
    let named = |kind: &str, name: String, known: Vec<&str>| {
        let f = base(Code::UnknownName)
            .with("kind_key", format!("quality.lint.kind.{kind}"))
            .with("name", name.clone());
        suggestion(f, &name, known)
    };
    match p.kind {
        ProblemKind::UnknownField(name) => {
            let mut known: Vec<&str> = schema.fields.keys().map(String::as_str).collect();
            known.extend(file.derived.keys().map(String::as_str));
            suggestion(
                base(Code::UnknownField).with("name", name.clone()),
                &name,
                known,
            )
        }
        ProblemKind::UnknownPopulation(name) => named(
            "population",
            name,
            file.populations.keys().map(String::as_str).collect(),
        ),
        ProblemKind::UnknownList(name) => named(
            "list",
            name,
            file.lists.keys().map(String::as_str).collect(),
        ),
        ProblemKind::UnknownTable(name) => named(
            "table",
            name,
            file.tables.keys().map(String::as_str).collect(),
        ),
        ProblemKind::UnknownReference(name) => named(
            "reference",
            name,
            file.references.keys().map(String::as_str).collect(),
        ),
        ProblemKind::Missing(parameter) => base(Code::Missing).with("parameter", parameter),
        ProblemKind::NotApplicable(parameter) => {
            let pattern = super::rule_of(file, &p.rule)
                .and_then(|r| r.check.as_ref())
                .map(|c| format!("{:?}", c.value).to_lowercase());
            base(Code::NotApplicable)
                .with("parameter", parameter)
                .with("pattern", pattern.unwrap_or_default())
        }
        ProblemKind::BadValue { parameter, value } => base(Code::BadValue)
            .with("parameter", parameter)
            .with("value", value),
        ProblemKind::NotSupported(what) => {
            base(Code::NotSupported).with("what_key", format!("quality.lint.unsupported.{what}"))
        }
        ProblemKind::Expression(e) => {
            let column = e.at.map_or(String::new(), |(_, c)| c.to_string());
            let (key, detail) = reason(&e.reason);
            base(Code::Expression)
                .with("reason_key", key)
                .with("detail", detail)
                .with("column", column)
        }
    }
}

fn suggestion(f: Finding, name: &str, known: Vec<&str>) -> Finding {
    match closest(name, known) {
        Some(s) => f.with("suggestion", s),
        None => f,
    }
}

/// Why an expression was refused: the key of its text (values named
/// `*_key` are keys the caller translates first) and the detail it names.
fn reason(r: &Reason) -> (String, String) {
    let name = |f| quality_rules_tree::function_name(f).to_string();
    let (id, detail) = match r {
        Reason::Syntax(s) => ("syntax", s.clone()),
        Reason::TrailingInput => ("trailing_input", String::new()),
        Reason::NotAllowed(what) => ("not_allowed", what.clone()),
        Reason::UnknownFunction(f) => ("unknown_function", f.clone()),
        Reason::Arity { function, got } => ("arity", format!("{} ({got})", name(*function))),
        Reason::TextRequired { function } => ("text_required", name(*function)),
        Reason::UnknownParameter(p) => ("unknown_parameter", p.clone()),
        Reason::ControlCharacter => ("control_character", String::new()),
        Reason::BadInterval => ("bad_interval", String::new()),
        Reason::UnsupportedType(t) => ("unsupported_type", t.clone()),
        Reason::TooDeep => ("too_deep", String::new()),
        Reason::TooLarge => ("too_large", String::new()),
    };
    (format!("quality.lint.reason.{id}"), detail)
}

/// Every reason's key, for the test that each has its text.
#[cfg(test)]
pub(crate) const REASONS: [&str; 12] = [
    "syntax",
    "trailing_input",
    "not_allowed",
    "unknown_function",
    "arity",
    "text_required",
    "unknown_parameter",
    "control_character",
    "bad_interval",
    "unsupported_type",
    "too_deep",
    "too_large",
];
