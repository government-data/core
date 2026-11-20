//! Text comparisons as decision 0033 states them: trimmed (the no-break
//! space counted as a space) and case-insensitive; accents count.

use super::FieldType;
use quality_rules_tree::{BinaryOp, Expr, Function, Literal};

/// Rebuilds a tree bottom-up, letting `f` replace each node.
pub(super) fn rewrite<F: FnMut(Expr) -> Expr>(e: Expr, f: &mut F) -> Expr {
    let rebuilt = match e {
        Expr::Not(x) => Expr::Not(Box::new(rewrite(*x, f))),
        Expr::Negate(x) => Expr::Negate(Box::new(rewrite(*x, f))),
        Expr::Binary { op, left, right } => Expr::Binary {
            op,
            left: Box::new(rewrite(*left, f)),
            right: Box::new(rewrite(*right, f)),
        },
        Expr::IsNull { expr, negated } => Expr::IsNull {
            expr: Box::new(rewrite(*expr, f)),
            negated,
        },
        Expr::InList {
            expr,
            list,
            negated,
        } => {
            let expr = Box::new(rewrite(*expr, f));
            Expr::InList {
                expr,
                list: list.into_iter().map(|x| rewrite(x, f)).collect(),
                negated,
            }
        }
        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => Expr::Between {
            expr: Box::new(rewrite(*expr, f)),
            low: Box::new(rewrite(*low, f)),
            high: Box::new(rewrite(*high, f)),
            negated,
        },
        Expr::Like {
            expr,
            pattern,
            negated,
        } => Expr::Like {
            expr: Box::new(rewrite(*expr, f)),
            pattern,
            negated,
        },
        Expr::Case {
            operand,
            branches,
            otherwise,
        } => Expr::Case {
            operand: operand.map(|o| Box::new(rewrite(*o, f))),
            branches: branches
                .into_iter()
                .map(|(c, v)| (rewrite(c, f), rewrite(v, f)))
                .collect(),
            otherwise: otherwise.map(|o| Box::new(rewrite(*o, f))),
        },
        Expr::Cast { expr, to } => Expr::Cast {
            expr: Box::new(rewrite(*expr, f)),
            to,
        },
        Expr::Call { function, args } => Expr::Call {
            function,
            args: args.into_iter().map(|x| rewrite(x, f)).collect(),
        },
        Expr::Matches { expr, format } => Expr::Matches {
            expr: Box::new(rewrite(*expr, f)),
            format,
        },
        Expr::Normalized(x) => Expr::Normalized(Box::new(rewrite(*x, f))),
        Expr::Duplicated(key) => Expr::Duplicated(key.into_iter().map(|x| rewrite(x, f)).collect()),
        leaf @ (Expr::Field(_) | Expr::Literal(_) | Expr::Cut | Expr::Interval { .. }) => leaf,
    };
    f(rebuilt)
}

/// `upper(trim(x))`: how two texts are compared.
pub(super) fn norm(e: Expr) -> Expr {
    if let Expr::Call {
        function: Function::Upper,
        args,
    } = &e
    {
        if matches!(
            args.as_slice(),
            [Expr::Call {
                function: Function::Trim,
                ..
            }]
        ) {
            return e;
        }
    }
    let trimmed = Expr::Call {
        function: Function::Trim,
        args: vec![e],
    };
    Expr::Call {
        function: Function::Upper,
        args: vec![trimmed],
    }
}

/// Wraps both sides of every comparison that involves a text.
pub(super) fn normalize_comparisons(e: Expr, type_of: &impl Fn(&str) -> Option<FieldType>) -> Expr {
    rewrite(e, &mut |node| match node {
        Expr::Binary { op, left, right }
            if is_comparison(op) && (is_text(&left, type_of) || is_text(&right, type_of)) =>
        {
            Expr::Binary {
                op,
                left: Box::new(norm(*left)),
                right: Box::new(norm(*right)),
            }
        }
        Expr::InList {
            expr,
            list,
            negated,
        } if is_text(&expr, type_of) || list.iter().any(|x| is_text(x, type_of)) => Expr::InList {
            expr: Box::new(norm(*expr)),
            list: list.into_iter().map(norm).collect(),
            negated,
        },
        Expr::Like {
            expr,
            pattern,
            negated,
        } if is_text(&expr, type_of) => Expr::Like {
            expr: Box::new(norm(*expr)),
            pattern: pattern.to_uppercase(),
            negated,
        },
        other => other,
    })
}

fn is_comparison(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Eq
            | BinaryOp::NotEq
            | BinaryOp::Lt
            | BinaryOp::LtEq
            | BinaryOp::Gt
            | BinaryOp::GtEq
    )
}

fn is_text(e: &Expr, type_of: &impl Fn(&str) -> Option<FieldType>) -> bool {
    match e {
        Expr::Field(name) => matches!(type_of(name), Some(FieldType::Text | FieldType::Email)),
        Expr::Literal(Literal::Text(_)) => true,
        Expr::Call {
            function: Function::Upper | Function::Lower | Function::Trim,
            ..
        } => true,
        Expr::Binary {
            op: BinaryOp::Concat,
            ..
        } => true,
        _ => false,
    }
}
