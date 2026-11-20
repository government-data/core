//! The patterns of one field's value: `placeholder`, `length`, `format`,
//! `domain`, `range` and `fresh` (decision 0033, point 1), and the fillers
//! `required` also counts as empty.

use super::context::Context;
use super::cross;
use super::expand::Unit;
use super::patterns::{binary, not};
use super::text::norm;
use super::{or, FieldType, ProblemKind};
use quality_rules_model::{At, Position};
use quality_rules_model::{Rule, Text, ValueSet};
use quality_rules_tree::{BinaryOp, Expr, Function, IntervalUnit, Literal};

/// The dataset's fillers for a field of this type.
pub(super) fn placeholders(ctx: &Context<'_>, t: FieldType) -> Vec<Text> {
    let p = &ctx.file.placeholders;
    match t {
        FieldType::Text => p.text.clone(),
        FieldType::Email => p.text.iter().chain(&p.email).cloned().collect(),
        FieldType::Number => p.number.clone(),
        FieldType::Date => p.date.clone(),
        FieldType::Boolean => Vec::new(),
    }
}

/// The value is one of these fillers; `*` in an e-mail filler is any text.
pub(super) fn placeholder_test(
    ctx: &mut Context<'_>,
    rule: &Rule,
    f: &Expr,
    t: FieldType,
    values: &[Text],
) -> Option<Expr> {
    let (patterns, exact): (Vec<&Text>, Vec<&Text>) =
        values.iter().partition(|v| v.0.contains('*'));
    let mut tests = Vec::new();
    if !exact.is_empty() {
        let list = exact
            .iter()
            .filter_map(|v| literal(ctx, rule, t, v, "values", rule.check().at))
            .collect();
        tests.push(ctx.normalized(
            rule,
            Expr::InList {
                expr: Box::new(f.clone()),
                list,
                negated: false,
            },
        ));
    }
    for p in patterns {
        let escaped =
            p.0.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
                .replace('*', "%");
        tests.push(Expr::Like {
            expr: Box::new(norm(f.clone())),
            pattern: escaped.to_uppercase(),
            negated: false,
        });
    }
    tests.into_iter().reduce(or)
}

pub(super) fn placeholder(
    ctx: &mut Context<'_>,
    rule: &Rule,
    f: &Expr,
    t: FieldType,
) -> Option<Expr> {
    let values = rule
        .values
        .as_ref()
        .map_or_else(|| placeholders(ctx, t), |v| v.value.clone());
    let test = placeholder_test(ctx, rule, f, t, &values);
    if test.is_none() {
        ctx.report(
            &rule.id.value,
            rule.check().at,
            ProblemKind::Missing("values"),
        );
    }
    test
}

/// A value of the rule typed as its field: numbers and dates checked here.
pub(super) fn literal(
    ctx: &mut Context<'_>,
    rule: &Rule,
    t: FieldType,
    v: &Text,
    parameter: &'static str,
    at: Position,
) -> Option<Expr> {
    let s = v.0.trim();
    let lit = match t {
        FieldType::Number => s
            .parse::<f64>()
            .is_ok()
            .then(|| Literal::Number(s.to_string())),
        FieldType::Date => is_date(s).then(|| Literal::Date(s.to_string())),
        FieldType::Boolean => s.parse::<bool>().ok().map(Literal::Boolean),
        FieldType::Text | FieldType::Email => Some(Literal::Text(v.0.clone())),
    };
    if lit.is_none() {
        ctx.report(
            &rule.id.value,
            at,
            ProblemKind::BadValue {
                parameter,
                value: v.0.clone(),
            },
        );
    }
    lit.map(Expr::Literal)
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

pub(super) fn length(ctx: &mut Context<'_>, rule: &Rule, f: &Expr) -> Option<Expr> {
    let len = Expr::Call {
        function: Function::Length,
        args: vec![Expr::Call {
            function: Function::Trim,
            args: vec![f.clone()],
        }],
    };
    let mut tests = Vec::new();
    if let Some(set) = &rule.in_ {
        let ValueSet::Values(values) = &set.value else {
            ctx.report(
                &rule.id.value,
                set.at,
                ProblemKind::BadValue {
                    parameter: "in",
                    value: "a list or a reference".into(),
                },
            );
            return None;
        };
        let list = values
            .iter()
            .filter_map(|v| literal(ctx, rule, FieldType::Number, v, "in", set.at))
            .collect();
        tests.push(Expr::InList {
            expr: Box::new(len.clone()),
            list,
            negated: true,
        });
    }
    for (bound, op, name) in [
        (&rule.min, BinaryOp::Lt, "min"),
        (&rule.max, BinaryOp::Gt, "max"),
    ] {
        if let Some(b) = bound {
            tests.push(binary(
                op,
                len.clone(),
                literal(ctx, rule, FieldType::Number, &b.value, name, b.at)?,
            ));
        }
    }
    let fails = tests.into_iter().reduce(or);
    if fails.is_none() {
        ctx.report(&rule.id.value, rule.check().at, ProblemKind::Missing("in"));
    }
    fails
}

pub(super) fn format(ctx: &mut Context<'_>, rule: &Rule, f: &Expr) -> Option<Expr> {
    let trimmed = Expr::Call {
        function: Function::Trim,
        args: vec![f.clone()],
    };
    match (&rule.pattern, &rule.named) {
        (Some(p), None) => Some(not(Expr::Call {
            function: Function::RegexpLike,
            args: vec![trimmed, Expr::Literal(Literal::Text(p.value.clone()))],
        })),
        (None, Some(n)) => Some(not(Expr::Matches {
            expr: Box::new(trimmed),
            format: n.value,
        })),
        (Some(_), Some(n)) => {
            ctx.report(&rule.id.value, n.at, ProblemKind::NotApplicable("named"));
            None
        }
        (None, None) => {
            ctx.report(
                &rule.id.value,
                rule.check().at,
                ProblemKind::Missing("pattern"),
            );
            None
        }
    }
}

pub(super) fn domain(ctx: &mut Context<'_>, rule: &Rule, f: &Expr, t: FieldType) -> Option<Expr> {
    let (set, negated) = match (&rule.in_, &rule.not_in) {
        (Some(set), None) => (set, true),
        (None, Some(set)) => (set, false),
        (Some(_), Some(other)) => {
            ctx.report(
                &rule.id.value,
                other.at,
                ProblemKind::NotApplicable("not_in"),
            );
            return None;
        }
        (None, None) => {
            ctx.report(&rule.id.value, rule.check().at, ProblemKind::Missing("in"));
            return None;
        }
    };
    let values = match &set.value {
        ValueSet::Values(v) => v.clone(),
        ValueSet::List { list } => ctx.list(rule, list, set.at)?,
        ValueSet::Reference { reference } => {
            return cross::in_reference(ctx, rule, f, reference, set.at, negated)
        }
    };
    let list = values
        .iter()
        .filter_map(|v| literal(ctx, rule, t, v, "in", set.at))
        .collect();
    Some(ctx.normalized(
        rule,
        Expr::InList {
            expr: Box::new(f.clone()),
            list,
            negated,
        },
    ))
}

pub(super) fn range(
    ctx: &mut Context<'_>,
    rule: &Rule,
    unit: &Unit,
    f: &Expr,
    t: FieldType,
) -> Option<Expr> {
    let exclusive = |b: &Option<At<bool>>| b.as_ref().is_some_and(|b| b.value);
    let mut tests = Vec::new();
    let bounds = [
        (
            &rule.min,
            if exclusive(&rule.min_exclusive) {
                BinaryOp::LtEq
            } else {
                BinaryOp::Lt
            },
        ),
        (
            &rule.max,
            if exclusive(&rule.max_exclusive) {
                BinaryOp::GtEq
            } else {
                BinaryOp::Gt
            },
        ),
    ];
    for (bound, op) in bounds {
        let Some(b) = bound else { continue };
        let value = if t == FieldType::Date && is_date(b.value.0.trim()) {
            Expr::Literal(Literal::Date(b.value.0.trim().to_string()))
        } else {
            ctx.expr(
                rule,
                unit,
                &At {
                    value: b.value.0.clone(),
                    at: b.at,
                },
            )?
        };
        tests.push(binary(op, f.clone(), value));
    }
    let fails = tests.into_iter().reduce(or);
    if fails.is_none() {
        ctx.report(&rule.id.value, rule.check().at, ProblemKind::Missing("min"));
    }
    fails
}

/// `max_age: 13 months`: older than that against the cut.
pub(super) fn fresh(ctx: &mut Context<'_>, rule: &Rule, f: &Expr) -> Option<Expr> {
    let Some(age) = &rule.max_age else {
        ctx.report(
            &rule.id.value,
            rule.check().at,
            ProblemKind::Missing("max_age"),
        );
        return None;
    };
    let mut words = age.value.split_whitespace();
    let amount = words.next().and_then(|n| n.parse::<i64>().ok());
    let unit = match words.next().map(|u| u.trim_end_matches('s')) {
        Some("day") => Some(IntervalUnit::Day),
        Some("month") => Some(IntervalUnit::Month),
        Some("year") => Some(IntervalUnit::Year),
        _ => None,
    };
    let (Some(amount), Some(unit), None) = (amount, unit, words.next()) else {
        ctx.report(
            &rule.id.value,
            age.at,
            ProblemKind::BadValue {
                parameter: "max_age",
                value: age.value.clone(),
            },
        );
        return None;
    };
    let oldest = binary(
        BinaryOp::Subtract,
        Expr::Cut,
        Expr::Interval { amount, unit },
    );
    Some(binary(BinaryOp::Lt, f.clone(), oldest))
}
