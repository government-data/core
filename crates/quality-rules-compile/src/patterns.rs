//! Each pattern's failure as a tree (decision 0033, points 1 and 4): the
//! patterns of one field here, those across fields in `cross`.

use super::context::Context;
use super::cross;
use super::expand::Unit;
use super::field::{
    domain, format, fresh, length, placeholder, placeholder_test, placeholders, range,
};
use super::{and, or, params, Compiled, ProblemKind};
use quality_rules_model::{Dimension, OnEmpty, Pattern, Rule, Severity};
use quality_rules_tree::{BinaryOp, Expr, Function, Literal};

/// What a pattern's lowering returns: its failure, and the field whose
/// emptiness skips it (or several, all skipping).
pub(super) struct Lowered {
    pub(super) fails: Expr,
    pub(super) skips_on: Vec<Expr>,
    pub(super) field: Option<String>,
}

pub(super) fn compile_one(ctx: &mut Context<'_>, rule: &Rule, unit: &Unit) -> Option<Compiled> {
    let misplaced = params::misplaced(rule);
    let clean = misplaced.is_empty();
    for (at, kind) in misplaced {
        ctx.report(&rule.id.value, at, kind);
    }
    let applies = applies(ctx, rule, unit);
    let lowered = lower(ctx, rule, unit);
    let (applies, lowered) = (applies?, lowered?);
    if !clean {
        return None;
    }
    let pattern = rule.check().value;
    let fail_on_empty = rule
        .on_empty
        .as_ref()
        .is_some_and(|o| o.value == OnEmpty::Fail);
    let empties = lowered.skips_on.into_iter().map(is_empty).reduce(or);
    let (fails, skips) = match (fail_on_empty, empties) {
        (true, Some(empty)) => (or(empty, lowered.fails), None),
        (_, empties) => (lowered.fails, empties),
    };
    Some(Compiled {
        id: unit.id.clone(),
        field: lowered.field,
        pattern,
        dimension: dimension(rule)?,
        severity: rule
            .severity
            .as_ref()
            .map_or_else(|| default_severity(pattern), |s| s.value),
        applies,
        skips,
        fails,
    })
}

/// The population and the condition, both of which must hold.
fn applies(ctx: &mut Context<'_>, rule: &Rule, unit: &Unit) -> Option<Expr> {
    let mut parts = Vec::new();
    if let Some(pop) = &rule.population {
        match ctx.file.populations.get(&pop.value).cloned() {
            Some(text) => parts.push(ctx.expr(rule, unit, &text)?),
            None => {
                ctx.report(
                    &rule.id.value,
                    pop.at,
                    ProblemKind::UnknownPopulation(pop.value.clone()),
                );
                return None;
            }
        }
    }
    if let Some(when) = &rule.when {
        parts.push(ctx.expr(rule, unit, when)?);
    }
    Some(
        parts
            .into_iter()
            .reduce(and)
            .unwrap_or(Expr::Literal(Literal::Boolean(true))),
    )
}

fn lower(ctx: &mut Context<'_>, rule: &Rule, unit: &Unit) -> Option<Lowered> {
    let pattern = rule.check().value;
    if matches!(
        pattern,
        Pattern::Combination | Pattern::Implies | Pattern::Expression
    ) {
        return cross::lower_across(ctx, rule, unit);
    }
    let Some((name, at)) = unit.field.clone() else {
        ctx.report(
            &rule.id.value,
            rule.check().at,
            ProblemKind::Missing("field"),
        );
        return None;
    };
    let (f, t) = ctx.field(rule, &name, at)?;
    let fails = match pattern {
        Pattern::Required => {
            let filler = placeholder_test(ctx, rule, &f, t, &placeholders(ctx, t));
            let empty = is_empty(f.clone());
            return Some(Lowered {
                fails: filler.map_or(empty.clone(), |p| or(empty, p)),
                skips_on: vec![],
                field: Some(name),
            });
        }
        Pattern::Empty => {
            return Some(Lowered {
                fails: Expr::Not(Box::new(is_empty(f))),
                skips_on: vec![],
                field: Some(name),
            });
        }
        Pattern::Placeholder => placeholder(ctx, rule, &f, t)?,
        Pattern::Length => length(ctx, rule, &f)?,
        Pattern::Format => format(ctx, rule, &f)?,
        Pattern::Domain => domain(ctx, rule, &f, t)?,
        Pattern::Range => range(ctx, rule, unit, &f, t)?,
        Pattern::Fresh => fresh(ctx, rule, &f)?,
        Pattern::Sum | Pattern::Unique | Pattern::Differs => {
            return cross::lower_field(ctx, rule, unit, (f, t, name))
        }
        Pattern::Combination | Pattern::Implies | Pattern::Expression => {
            unreachable!("handled above")
        }
    };
    Some(Lowered {
        fails,
        skips_on: vec![f],
        field: Some(name),
    })
}

pub(super) fn is_empty(e: Expr) -> Expr {
    Expr::Call {
        function: Function::IsEmpty,
        args: vec![e],
    }
}

pub(super) fn not(e: Expr) -> Expr {
    Expr::Not(Box::new(e))
}

pub(super) fn binary(op: BinaryOp, left: Expr, right: Expr) -> Expr {
    Expr::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn dimension(rule: &Rule) -> Option<Dimension> {
    Some(match rule.check().value {
        Pattern::Required => Dimension::Completeness,
        Pattern::Empty
        | Pattern::Sum
        | Pattern::Combination
        | Pattern::Implies
        | Pattern::Differs => Dimension::Consistency,
        Pattern::Placeholder | Pattern::Range => Dimension::Accuracy,
        Pattern::Length | Pattern::Format | Pattern::Domain => Dimension::Validity,
        Pattern::Fresh => Dimension::Timeliness,
        Pattern::Unique => Dimension::Uniqueness,
        Pattern::Expression => rule.dimension.as_ref()?.value,
    })
}

fn default_severity(pattern: Pattern) -> Severity {
    match pattern {
        Pattern::Required => Severity::Blocking,
        Pattern::Unique | Pattern::Differs => Severity::Minor,
        _ => Severity::Major,
    }
}
