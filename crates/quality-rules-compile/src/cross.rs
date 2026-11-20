//! The patterns that look beyond one value: other fields (`sum`,
//! `combination`, `implies`, `differs`, `expression`), other records
//! (`unique`) or another dataset (`domain` against a reference).

use super::context::Context;
use super::expand::Unit;
use super::field::literal;
use super::patterns::{binary, not, Lowered};
use super::{and, or, FieldType, ProblemKind};
use quality_rules_model::{At, Position};
use quality_rules_model::{OneOrMany, Pattern, Rule, Text};
use quality_rules_tree::{BinaryOp, Expr, Function, Literal};

/// `combination`, `implies` and `expression`: their field is optional.
pub(super) fn lower_across(ctx: &mut Context<'_>, rule: &Rule, unit: &Unit) -> Option<Lowered> {
    let field = unit.field.as_ref().map(|f| f.0.clone());
    match rule.check().value {
        Pattern::Combination => combination(ctx, rule),
        Pattern::Implies => {
            let (Some(if_), Some(then)) = (&rule.if_, &rule.then) else {
                let missing = if rule.if_.is_none() { "if" } else { "then" };
                ctx.report(
                    &rule.id.value,
                    rule.check().at,
                    ProblemKind::Missing(missing),
                );
                return None;
            };
            let (cond, then) = (ctx.expr(rule, unit, if_)?, ctx.expr(rule, unit, then)?);
            let mut fails = and(cond.clone(), not(then.clone()));
            if rule.both_ways.as_ref().is_some_and(|b| b.value) {
                fails = or(fails, and(then, not(cond)));
            }
            Some(Lowered {
                fails,
                skips_on: vec![],
                field,
            })
        }
        Pattern::Expression => {
            if rule.dimension.is_none() {
                ctx.report(
                    &rule.id.value,
                    rule.check().at,
                    ProblemKind::Missing("dimension"),
                );
            }
            let Some(text) = &rule.fails else {
                ctx.report(
                    &rule.id.value,
                    rule.check().at,
                    ProblemKind::Missing("fails"),
                );
                return None;
            };
            let fails = ctx.expr(rule, unit, text)?;
            rule.dimension.as_ref()?;
            Some(Lowered {
                fails,
                skips_on: vec![],
                field,
            })
        }
        _ => unreachable!("only the patterns across fields come here"),
    }
}

/// `sum`, `unique` and `differs`, on the rule's field.
pub(super) fn lower_field(
    ctx: &mut Context<'_>,
    rule: &Rule,
    unit: &Unit,
    (f, t, name): (Expr, FieldType, String),
) -> Option<Lowered> {
    let (fails, mut skips_on) = match rule.check().value {
        Pattern::Sum => (sum(ctx, rule, unit, &f)?, vec![]),
        Pattern::Unique => (unique(ctx, rule, unit, &f, t)?, vec![]),
        Pattern::Differs => {
            let Some(from) = &rule.from else {
                ctx.report(
                    &rule.id.value,
                    rule.check().at,
                    ProblemKind::Missing("from"),
                );
                return None;
            };
            let (other, _) = ctx.field(rule, &unit.subst(&from.value), from.at)?;
            let (a, b) = if normalize(rule) {
                (normalized(&f), normalized(&other))
            } else {
                (f.clone(), other.clone())
            };
            (
                ctx.normalized(rule, binary(BinaryOp::Eq, a, b)),
                vec![other],
            )
        }
        _ => unreachable!("only sum, unique and differs come here"),
    };
    skips_on.insert(0, f);
    Some(Lowered {
        fails,
        skips_on,
        field: Some(name),
    })
}

fn normalize(rule: &Rule) -> bool {
    rule.normalize.as_ref().is_some_and(|n| n.value)
}

fn normalized(e: &Expr) -> Expr {
    Expr::Normalized(Box::new(e.clone()))
}

/// The total differs from its signed parts by more than the tolerance.
fn sum(ctx: &mut Context<'_>, rule: &Rule, unit: &Unit, f: &Expr) -> Option<Expr> {
    let Some(parts) = &rule.parts else {
        ctx.report(
            &rule.id.value,
            rule.check().at,
            ProblemKind::Missing("parts"),
        );
        return None;
    };
    let mut total = None;
    for part in &parts.value {
        let e = ctx.expr(
            rule,
            unit,
            &At {
                value: part.clone(),
                at: parts.at,
            },
        )?;
        total = Some(match total {
            None => e,
            Some(t) => binary(BinaryOp::Add, t, e),
        });
    }
    let total = total?;
    let tolerance = match &rule.tolerance {
        Some(t) => literal(ctx, rule, FieldType::Number, &t.value, "tolerance", t.at)?,
        None => Expr::Literal(Literal::Number("0".into())),
    };
    let difference = Expr::Call {
        function: Function::Abs,
        args: vec![binary(BinaryOp::Subtract, f.clone(), total)],
    };
    Some(binary(BinaryOp::Gt, difference, tolerance))
}

/// Another record has the same key; `ignore` lists values that may repeat.
fn unique(ctx: &mut Context<'_>, rule: &Rule, unit: &Unit, f: &Expr, t: FieldType) -> Option<Expr> {
    let mut key = Vec::new();
    match &rule.key {
        Some(k) => {
            for name in k.value.names() {
                key.push(ctx.field(rule, &unit.subst(name), k.at)?.0);
            }
        }
        None => key.push(f.clone()),
    }
    let key: Vec<Expr> = key
        .into_iter()
        .map(|e| {
            if normalize(rule) {
                normalized(&e)
            } else {
                super::text::norm(e)
            }
        })
        .collect();
    let mut fails = Expr::Duplicated(key);
    if let Some(ignore) = &rule.ignore {
        let list = ignore
            .value
            .iter()
            .filter_map(|v| literal(ctx, rule, t, v, "ignore", ignore.at))
            .collect();
        let ignored = ctx.normalized(
            rule,
            Expr::InList {
                expr: Box::new(f.clone()),
                list,
                negated: false,
            },
        );
        fails = and(fails, not(ignored));
    }
    Some(fails)
}

/// `domain` against another dataset's column, declared in `references`.
pub(super) fn in_reference(
    ctx: &mut Context<'_>,
    rule: &Rule,
    f: &Expr,
    reference: &str,
    at: Position,
    negated: bool,
) -> Option<Expr> {
    let name = reference.split('.').next().unwrap_or_default();
    if !ctx.file.references.contains_key(name) || !reference.contains('.') {
        ctx.report(
            &rule.id.value,
            at,
            ProblemKind::UnknownReference(reference.to_string()),
        );
        return None;
    }
    let found = Expr::Call {
        function: Function::InReference,
        args: vec![f.clone(), Expr::Literal(Literal::Text(reference.into()))],
    };
    Some(if negated { not(found) } else { found })
}

/// A record fails once, whichever row of the table it breaks.
fn combination(ctx: &mut Context<'_>, rule: &Rule) -> Option<Lowered> {
    let Some(name) = &rule.table else {
        ctx.report(
            &rule.id.value,
            rule.check().at,
            ProblemKind::Missing("table"),
        );
        return None;
    };
    let Some(table) = ctx.file.tables.get(&name.value).cloned() else {
        ctx.report(
            &rule.id.value,
            name.at,
            ProblemKind::UnknownTable(name.value.clone()),
        );
        return None;
    };
    let [when_name] = table.when.value.names()[..] else {
        ctx.report(
            &rule.id.value,
            table.when.at,
            ProblemKind::NotSupported("table_on_several_fields"),
        );
        return None;
    };
    let (w, wt) = ctx.field(rule, when_name, table.when.at)?;
    let (then, tt) = ctx.field(rule, &table.then.value, table.then.at)?;
    let star = Text("*".into());
    let listed: Vec<Text> = table
        .rows
        .iter()
        .flat_map(|r| values(&r.value.when))
        .filter(|v| *v != star)
        .collect();
    let mut conditions = Vec::new();
    for row in &table.rows {
        let whens = values(&row.value.when);
        let matches = if whens.contains(&star) {
            in_values(ctx, rule, &w, wt, &listed, row.at, true)
        } else {
            in_values(ctx, rule, &w, wt, &whens, row.at, false)
        };
        if !row.value.allowed.is_empty() {
            conditions.push(and(
                matches.clone(),
                in_values(ctx, rule, &then, tt, &row.value.allowed, row.at, true),
            ));
        }
        if !row.value.forbidden.is_empty() {
            conditions.push(and(
                matches,
                in_values(ctx, rule, &then, tt, &row.value.forbidden, row.at, false),
            ));
        }
    }
    let fails = conditions.into_iter().reduce(or)?;
    Some(Lowered {
        fails,
        skips_on: vec![then, w],
        field: Some(table.then.value.clone()),
    })
}

fn values(v: &OneOrMany) -> Vec<Text> {
    match v {
        OneOrMany::One(t) => vec![t.clone()],
        OneOrMany::Many(ts) => ts.clone(),
    }
}

fn in_values(
    ctx: &mut Context<'_>,
    rule: &Rule,
    e: &Expr,
    t: FieldType,
    vs: &[Text],
    at: Position,
    negated: bool,
) -> Expr {
    let list = vs
        .iter()
        .filter_map(|v| literal(ctx, rule, t, v, "rows", at))
        .collect();
    ctx.normalized(
        rule,
        Expr::InList {
            expr: Box::new(e.clone()),
            list,
            negated,
        },
    )
}
