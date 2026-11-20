//! Which parameters each pattern takes (decision 0033, point 1). The model
//! accepts all of them; a parameter the pattern does not take is reported
//! at its position, so nothing written is silently ignored.

use super::ProblemKind;
use quality_rules_model::Position;
use quality_rules_model::{Pattern, Rule};

/// The pattern-specific parameters set on a rule, with their positions.
fn present(rule: &Rule) -> Vec<(&'static str, Position)> {
    let p = |name: &'static str, at: Option<Position>| at.map(|at| (name, at));
    [
        p("values", rule.values.as_ref().map(|v| v.at)),
        p("in", rule.in_.as_ref().map(|v| v.at)),
        p("not_in", rule.not_in.as_ref().map(|v| v.at)),
        p("min", rule.min.as_ref().map(|v| v.at)),
        p("max", rule.max.as_ref().map(|v| v.at)),
        p("min_exclusive", rule.min_exclusive.as_ref().map(|v| v.at)),
        p("max_exclusive", rule.max_exclusive.as_ref().map(|v| v.at)),
        p("pattern", rule.pattern.as_ref().map(|v| v.at)),
        p("named", rule.named.as_ref().map(|v| v.at)),
        p("max_age", rule.max_age.as_ref().map(|v| v.at)),
        p("parts", rule.parts.as_ref().map(|v| v.at)),
        p("tolerance", rule.tolerance.as_ref().map(|v| v.at)),
        p("key", rule.key.as_ref().map(|v| v.at)),
        p("normalize", rule.normalize.as_ref().map(|v| v.at)),
        p("ignore", rule.ignore.as_ref().map(|v| v.at)),
        p("table", rule.table.as_ref().map(|v| v.at)),
        p("if", rule.if_.as_ref().map(|v| v.at)),
        p("then", rule.then.as_ref().map(|v| v.at)),
        p("both_ways", rule.both_ways.as_ref().map(|v| v.at)),
        p("from", rule.from.as_ref().map(|v| v.at)),
        p("fails", rule.fails.as_ref().map(|v| v.at)),
        p("dimension", rule.dimension.as_ref().map(|v| v.at)),
        p("on_empty", rule.on_empty.as_ref().map(|v| v.at)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn takes(pattern: Pattern) -> &'static [&'static str] {
    match pattern {
        Pattern::Required | Pattern::Empty => &[],
        Pattern::Placeholder => &["values", "on_empty"],
        Pattern::Length => &["in", "min", "max", "on_empty"],
        Pattern::Format => &["pattern", "named", "on_empty"],
        Pattern::Domain => &["in", "not_in", "on_empty"],
        Pattern::Range => &["min", "max", "min_exclusive", "max_exclusive", "on_empty"],
        Pattern::Fresh => &["max_age", "on_empty"],
        Pattern::Sum => &["parts", "tolerance", "on_empty"],
        Pattern::Unique => &["key", "normalize", "ignore", "on_empty"],
        Pattern::Combination => &["table"],
        Pattern::Implies => &["if", "then", "both_ways"],
        Pattern::Differs => &["from", "normalize", "on_empty"],
        Pattern::Expression => &["fails", "dimension"],
    }
}

/// The parameters set on a rule that its pattern does not take.
pub(super) fn misplaced(rule: &Rule) -> Vec<(Position, ProblemKind)> {
    let allowed = takes(rule.check().value);
    present(rule)
        .into_iter()
        .filter(|(name, _)| !allowed.contains(name))
        .map(|(name, at)| (at, ProblemKind::NotApplicable(name)))
        .collect()
}
