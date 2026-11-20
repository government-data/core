//! The tree written in a neutral, SQL-like form: for tests, debugging and
//! explanations. It is not any engine's SQL; the dialects write that.

use std::fmt::{self, Display, Formatter};

use super::{BinaryOp, Expr, Function, IntervalUnit, Literal};

impl Display for Expr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Field(name) => write!(f, "{name}"),
            Expr::Literal(l) => write!(f, "{l}"),
            Expr::Cut => write!(f, ":cut"),
            Expr::Not(x) => write!(f, "NOT ({x})"),
            Expr::Negate(x) => write!(f, "-({x})"),
            Expr::Binary { op, left, right } => write!(f, "({left} {} {right})", op_text(*op)),
            Expr::IsNull { expr, negated } => write!(f, "{expr} IS {}NULL", not(*negated)),
            Expr::InList {
                expr,
                list,
                negated,
            } => write!(f, "{expr} {}IN ({})", not(*negated), join(list)),
            Expr::Between {
                expr,
                low,
                high,
                negated,
            } => write!(f, "{expr} {}BETWEEN {low} AND {high}", not(*negated)),
            Expr::Like {
                expr,
                pattern,
                negated,
            } => write!(
                f,
                "{expr} {}LIKE '{}'",
                not(*negated),
                pattern.replace('\'', "''")
            ),
            Expr::Case {
                operand,
                branches,
                otherwise,
            } => {
                write!(f, "CASE")?;
                if let Some(o) = operand {
                    write!(f, " {o}")?;
                }
                for (when, then) in branches {
                    write!(f, " WHEN {when} THEN {then}")?;
                }
                if let Some(o) = otherwise {
                    write!(f, " ELSE {o}")?;
                }
                write!(f, " END")
            }
            Expr::Cast { expr, to } => write!(f, "CAST({expr} AS {to:?})"),
            Expr::Interval { amount, unit } => write!(f, "INTERVAL {amount} {}", unit_text(*unit)),
            Expr::Call { function, args } => {
                write!(f, "{}({})", function_name(*function), join(args))
            }
            Expr::Matches { expr, format } => write!(f, "matches({expr}, {format:?})"),
            Expr::Normalized(x) => write!(f, "normalized({x})"),
            Expr::Duplicated(key) => write!(f, "duplicated({})", join(key)),
        }
    }
}

impl Display for Literal {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Text(s) => write!(f, "'{}'", s.replace('\'', "''")),
            Literal::Number(n) => write!(f, "{n}"),
            Literal::Boolean(b) => write!(f, "{}", if *b { "TRUE" } else { "FALSE" }),
            Literal::Null => write!(f, "NULL"),
            Literal::Date(d) => write!(f, "DATE '{d}'"),
        }
    }
}

fn not(negated: bool) -> &'static str {
    if negated {
        "NOT "
    } else {
        ""
    }
}

fn join(items: &[Expr]) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn op_text(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::And => "AND",
        BinaryOp::Or => "OR",
        BinaryOp::Eq => "=",
        BinaryOp::NotEq => "<>",
        BinaryOp::Lt => "<",
        BinaryOp::LtEq => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::GtEq => ">=",
        BinaryOp::Add => "+",
        BinaryOp::Subtract => "-",
        BinaryOp::Multiply => "*",
        BinaryOp::Divide => "/",
        BinaryOp::Modulo => "%",
        BinaryOp::Concat => "||",
    }
}

fn unit_text(unit: IntervalUnit) -> &'static str {
    match unit {
        IntervalUnit::Year => "YEAR",
        IntervalUnit::Month => "MONTH",
        IntervalUnit::Day => "DAY",
    }
}

/// The name a rule writes for each function.
pub fn function_name(function: Function) -> &'static str {
    match function {
        Function::IsEmpty => "is_empty",
        Function::RegexpLike => "regexp_like",
        Function::InList => "in_list",
        Function::InReference => "in_reference",
        Function::DaysSince => "days_since",
        Function::DaysBetween => "days_between",
        Function::YearsBetween => "years_between",
        Function::Upper => "upper",
        Function::Lower => "lower",
        Function::Trim => "trim",
        Function::Length => "length",
        Function::Coalesce => "coalesce",
        Function::Abs => "abs",
        Function::Round => "round",
    }
}
