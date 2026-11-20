//! The tree in words. Text comparisons are trimmed and case-insensitive by
//! rule (decision 0033, point 4), so `upper(trim(x))` reads as `x`.

use quality_rules_model::NamedFormat;
use quality_rules_tree::{BinaryOp, Expr, Function, IntervalUnit, Literal};
use texts::{text_in, Language};

/// How many values of a list are named before "and N more".
const LIST_SHOWN: usize = 5;

pub(crate) struct Words {
    pub(crate) language: Language,
}

impl Words {
    fn t(&self, key: &str, values: &[(&str, String)]) -> String {
        text_in(self.language, &format!("quality.explain.{key}"), values)
    }

    fn t1(&self, key: &str, name: &str, value: String) -> String {
        self.t(key, &[(name, value)])
    }

    pub(crate) fn expr(&self, e: &Expr) -> String {
        match e {
            Expr::Field(name) => name.clone(),
            Expr::Literal(l) => self.literal(l),
            Expr::Cut => self.t("cut", &[]),
            Expr::Not(x) => self.not(x),
            Expr::Negate(x) => format!("-{}", self.operand(x)),
            Expr::Binary { op, left, right } => self.binary(*op, left, right),
            Expr::IsNull { expr, negated } => self.t1(
                if *negated { "has_value" } else { "no_value" },
                "x",
                self.expr(expr),
            ),
            Expr::InList {
                expr,
                list,
                negated,
            } => {
                let key = if *negated { "not_one_of" } else { "one_of" };
                self.t(key, &[("x", self.expr(expr)), ("values", self.list(list))])
            }
            Expr::Between {
                expr,
                low,
                high,
                negated,
            } => self.t(
                if *negated { "not_between" } else { "between" },
                &[
                    ("x", self.expr(expr)),
                    ("low", self.expr(low)),
                    ("high", self.expr(high)),
                ],
            ),
            Expr::Like {
                expr,
                pattern,
                negated,
            } => {
                let key = if *negated { "not_like" } else { "like" };
                self.t(key, &[("x", self.expr(expr)), ("pattern", pattern.clone())])
            }
            Expr::Case { .. } => e.to_string(),
            Expr::Cast { expr, .. } => self.expr(expr),
            Expr::Interval { amount, unit } => self.interval(*amount, *unit),
            Expr::Call { function, args } => self.call(*function, args),
            Expr::Matches { expr, format } => self.t(
                "matches",
                &[
                    ("x", self.expr(expr)),
                    (
                        "format",
                        self.t(&format!("format.{}", format_id(*format)), &[]),
                    ),
                ],
            ),
            Expr::Normalized(x) => self.t1("normalized", "x", self.expr(x)),
            Expr::Duplicated(key) => self.t1("duplicated", "key", self.and_list(key)),
        }
    }

    fn literal(&self, l: &Literal) -> String {
        match l {
            Literal::Text(s) => format!("«{s}»"),
            Literal::Number(n) => n.clone(),
            Literal::Boolean(b) => self.t(if *b { "true" } else { "false" }, &[]),
            Literal::Null => self.t("null", &[]),
            Literal::Date(d) => d.clone(),
        }
    }

    /// The negations that read better as one phrase.
    fn not(&self, x: &Expr) -> String {
        match x {
            Expr::Not(inner) => self.expr(inner),
            Expr::Call {
                function: Function::IsEmpty,
                args,
            } => self.t1("filled", "x", self.expr(&args[0])),
            Expr::Call {
                function: Function::RegexpLike,
                args,
            } => self.t(
                "not_shaped",
                &[("x", self.expr(&args[0])), ("pattern", pattern(&args[1]))],
            ),
            Expr::Call {
                function: Function::InReference,
                args,
            } => self.t(
                "not_in_reference",
                &[("x", self.expr(&args[0])), ("reference", pattern(&args[1]))],
            ),
            Expr::Matches { expr, format } => self.t(
                "not_matches",
                &[
                    ("x", self.expr(expr)),
                    (
                        "format",
                        self.t(&format!("format.{}", format_id(*format)), &[]),
                    ),
                ],
            ),
            other => self.t1("not", "x", self.expr(other)),
        }
    }

    fn binary(&self, op: BinaryOp, left: &Expr, right: &Expr) -> String {
        let key = match op {
            BinaryOp::And => {
                return self.t(
                    "and",
                    &[("a", self.side(left, op)), ("b", self.side(right, op))],
                )
            }
            BinaryOp::Or => {
                return self.t(
                    "or",
                    &[("a", self.side(left, op)), ("b", self.side(right, op))],
                )
            }
            BinaryOp::Eq => "eq",
            BinaryOp::NotEq => "not_eq",
            BinaryOp::Lt => "lt",
            BinaryOp::LtEq => "lt_eq",
            BinaryOp::Gt => "gt",
            BinaryOp::GtEq => "gt_eq",
            BinaryOp::Add
            | BinaryOp::Subtract
            | BinaryOp::Multiply
            | BinaryOp::Divide
            | BinaryOp::Modulo
            | BinaryOp::Concat => {
                return format!(
                    "{} {} {}",
                    self.operand(left),
                    symbol(op),
                    self.operand(right)
                );
            }
        };
        self.t(key, &[("a", self.expr(left)), ("b", self.expr(right))])
    }

    /// `(a or b)` inside an `and`, and the other way round, in parentheses.
    fn side(&self, e: &Expr, parent: BinaryOp) -> String {
        match e {
            Expr::Binary { op, .. }
                if matches!(op, BinaryOp::And | BinaryOp::Or) && *op != parent =>
            {
                format!("({})", self.expr(e))
            }
            _ => self.expr(e),
        }
    }

    fn operand(&self, e: &Expr) -> String {
        match e {
            Expr::Binary { .. } => format!("({})", self.expr(e)),
            _ => self.expr(e),
        }
    }

    fn list(&self, items: &[Expr]) -> String {
        let shown: Vec<String> = items
            .iter()
            .take(LIST_SHOWN)
            .map(|e| self.expr(e))
            .collect();
        let more = items.len().saturating_sub(LIST_SHOWN);
        if more == 0 {
            shown.join(", ")
        } else {
            self.t(
                "more",
                &[("values", shown.join(", ")), ("count", more.to_string())],
            )
        }
    }

    fn and_list(&self, items: &[Expr]) -> String {
        items
            .iter()
            .map(|e| self.expr(e))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn interval(&self, amount: i64, unit: IntervalUnit) -> String {
        let key = match unit {
            IntervalUnit::Year => "years",
            IntervalUnit::Month => "months",
            IntervalUnit::Day => "days",
        };
        self.t1(key, "count", amount.to_string())
    }

    fn call(&self, function: Function, args: &[Expr]) -> String {
        let a = |i: usize| self.expr(&args[i]);
        match function {
            Function::IsEmpty => self.t1("empty", "x", a(0)),
            Function::RegexpLike => {
                self.t("shaped", &[("x", a(0)), ("pattern", pattern(&args[1]))])
            }
            Function::InReference => self.t(
                "in_reference",
                &[("x", a(0)), ("reference", pattern(&args[1]))],
            ),
            Function::InList => self.t("in_list", &[("x", a(0)), ("list", pattern(&args[1]))]),
            Function::DaysSince => self.t1("days_since", "x", a(0)),
            Function::DaysBetween => self.t("days_between", &[("a", a(0)), ("b", a(1))]),
            Function::YearsBetween => self.t("years_between", &[("a", a(0)), ("b", a(1))]),
            Function::Length => self.t1("length", "x", a(0)),
            Function::Abs => self.t1("abs", "x", a(0)),
            Function::Round => self.t1("round", "x", a(0)),
            Function::Coalesce => self.t(
                "coalesce",
                &[
                    ("a", a(0)),
                    (
                        "b",
                        args[1..]
                            .iter()
                            .map(|e| self.expr(e))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                ],
            ),
            Function::Upper | Function::Lower | Function::Trim => a(0),
        }
    }
}

/// A text argument as written (a pattern, a list's or reference's name).
fn pattern(e: &Expr) -> String {
    match e {
        Expr::Literal(Literal::Text(s)) => s.clone(),
        other => other.to_string(),
    }
}

fn symbol(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Subtract => "-",
        BinaryOp::Multiply => "×",
        BinaryOp::Divide => "÷",
        BinaryOp::Modulo => "mod",
        _ => "||",
    }
}

pub(crate) fn format_id(f: NamedFormat) -> &'static str {
    match f {
        NamedFormat::Email => "email",
        NamedFormat::MobileCo => "mobile_co",
        NamedFormat::Divipola => "divipola",
        NamedFormat::AddressCo => "address_co",
        NamedFormat::PersonName => "person_name",
        NamedFormat::NotOnlySymbols => "not_only_symbols",
        NamedFormat::Capitalized => "capitalized",
        NamedFormat::NormalizedSpaces => "normalized_spaces",
    }
}
