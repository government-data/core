//! From a rule's text to the neutral tree, node by node: `sqlparser` reads
//! standard SQL, and only the nodes of the closed grammar are kept
//! (decision 0019, point 2; 0033, point 6). The text is not kept: SQL is
//! written from the tree alone.

use sqlparser::ast::{self as sql, Spanned};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::Token;

use super::{BinaryOp, Expr, Function, IntervalUnit, Literal, Type};

/// How deep an expression may nest.
pub const MAX_DEPTH: usize = 32;
/// How many nodes an expression may have.
pub const MAX_NODES: usize = 1000;

/// Why an expression is refused, and where in its text (line, column).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprError {
    pub reason: Reason,
    pub at: Option<(u64, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    Syntax(String),
    /// Something after the expression: a second statement, a stray token.
    TrailingInput,
    /// A construct outside the grammar: a subquery, a qualified name, …
    NotAllowed(String),
    UnknownFunction(String),
    Arity {
        function: Function,
        got: usize,
    },
    /// The argument must be written as text (a list's name, a pattern).
    TextRequired {
        function: Function,
    },
    /// Only `:cut` exists.
    UnknownParameter(String),
    /// NUL or another control character inside a text.
    ControlCharacter,
    /// `INTERVAL n YEAR|MONTH|DAY`, with a whole `n`.
    BadInterval,
    UnsupportedType(String),
    TooDeep,
    TooLarge,
}

/// Reads one expression of the closed grammar.
pub fn parse_expr(text: &str) -> Result<Expr, ExprError> {
    let syntax = |e: sqlparser::parser::ParserError| ExprError {
        reason: Reason::Syntax(e.to_string()),
        at: None,
    };
    let mut parser = Parser::new(&GenericDialect {})
        .with_recursion_limit(MAX_DEPTH * 2)
        .try_with_sql(text)
        .map_err(syntax)?;
    let expr = parser.parse_expr().map_err(syntax)?;
    let next = parser.peek_token();
    if next.token != Token::EOF {
        let at = (next.span.start.line, next.span.start.column);
        return Err(ExprError {
            reason: Reason::TrailingInput,
            at: Some(at),
        });
    }
    Converter { nodes: 0 }.expr(&expr, 0)
}

struct Converter {
    nodes: usize,
}

fn refuse<T>(reason: Reason, node: &impl Spanned) -> Result<T, ExprError> {
    let start = node.span().start;
    let at = (start.line > 0).then_some((start.line, start.column));
    Err(ExprError { reason, at })
}

fn boxed(e: Expr) -> Box<Expr> {
    Box::new(e)
}

impl Converter {
    fn expr(&mut self, e: &sql::Expr, depth: usize) -> Result<Expr, ExprError> {
        self.nodes += 1;
        if depth > MAX_DEPTH {
            return refuse(Reason::TooDeep, e);
        }
        if self.nodes > MAX_NODES {
            return refuse(Reason::TooLarge, e);
        }
        let d = depth + 1;
        Ok(match e {
            sql::Expr::Identifier(ident) => Expr::Field(ident.value.clone()),
            sql::Expr::Value(v) => self.value(&v.value, e)?,
            sql::Expr::Nested(inner) => self.expr(inner, d)?,
            sql::Expr::UnaryOp { op, expr } => self.unary(*op, expr, d, e)?,
            sql::Expr::BinaryOp { left, op, right } => Expr::Binary {
                op: binary_op(op).map_or_else(
                    || refuse(Reason::NotAllowed(format!("operator {op}")), e),
                    Ok,
                )?,
                left: boxed(self.expr(left, d)?),
                right: boxed(self.expr(right, d)?),
            },
            sql::Expr::IsNull(x) => Expr::IsNull {
                expr: boxed(self.expr(x, d)?),
                negated: false,
            },
            sql::Expr::IsNotNull(x) => Expr::IsNull {
                expr: boxed(self.expr(x, d)?),
                negated: true,
            },
            sql::Expr::InList {
                expr,
                list,
                negated,
            } => Expr::InList {
                expr: boxed(self.expr(expr, d)?),
                list: list
                    .iter()
                    .map(|x| self.expr(x, d))
                    .collect::<Result<_, _>>()?,
                negated: *negated,
            },
            sql::Expr::Between {
                expr,
                negated,
                low,
                high,
            } => Expr::Between {
                expr: boxed(self.expr(expr, d)?),
                low: boxed(self.expr(low, d)?),
                high: boxed(self.expr(high, d)?),
                negated: *negated,
            },
            sql::Expr::Like {
                negated,
                any: false,
                expr,
                pattern,
                escape_char: None,
            } => Expr::Like {
                expr: boxed(self.expr(expr, d)?),
                pattern: self.text(pattern, d)?,
                negated: *negated,
            },
            sql::Expr::Case {
                operand,
                conditions,
                else_result,
                ..
            } => self.case(operand, conditions, else_result, d)?,
            sql::Expr::Cast {
                kind: sql::CastKind::Cast,
                expr,
                data_type,
                format: None,
                ..
            } => Expr::Cast {
                expr: boxed(self.expr(expr, d)?),
                to: cast_type(data_type, e)?,
            },
            sql::Expr::TypedString(ts) => typed_string(ts, e)?,
            sql::Expr::Interval(interval) => self.interval(interval, e)?,
            sql::Expr::Function(f) => self.function(f, d)?,
            // `trim(x)` has its own syntax in SQL: only the plain form.
            sql::Expr::Trim {
                expr,
                trim_where: None,
                trim_what: None,
                trim_characters: None,
            } => Expr::Call {
                function: Function::Trim,
                args: vec![self.expr(expr, d)?],
            },
            other => return refuse(Reason::NotAllowed(node_name(other)), other),
        })
    }

    fn value(&mut self, v: &sql::Value, node: &sql::Expr) -> Result<Expr, ExprError> {
        Ok(match v {
            sql::Value::SingleQuotedString(s) => Expr::Literal(Literal::Text(clean(s, node)?)),
            sql::Value::Number(n, false) => Expr::Literal(Literal::Number(n.clone())),
            sql::Value::Boolean(b) => Expr::Literal(Literal::Boolean(*b)),
            sql::Value::Null => Expr::Literal(Literal::Null),
            sql::Value::Placeholder(p) if p == ":cut" => Expr::Cut,
            sql::Value::Placeholder(p) => return refuse(Reason::UnknownParameter(p.clone()), node),
            other => return refuse(Reason::NotAllowed(format!("value {other}")), node),
        })
    }

    fn unary(
        &mut self,
        op: sql::UnaryOperator,
        x: &sql::Expr,
        d: usize,
        node: &sql::Expr,
    ) -> Result<Expr, ExprError> {
        match op {
            sql::UnaryOperator::Not => Ok(Expr::Not(boxed(self.expr(x, d)?))),
            sql::UnaryOperator::Minus => Ok(Expr::Negate(boxed(self.expr(x, d)?))),
            sql::UnaryOperator::Plus => self.expr(x, d),
            other => refuse(Reason::NotAllowed(format!("operator {other}")), node),
        }
    }

    /// A text that must be written as such: a `LIKE` pattern, a list's name.
    fn text(&mut self, e: &sql::Expr, d: usize) -> Result<String, ExprError> {
        match self.expr(e, d)? {
            Expr::Literal(Literal::Text(s)) => Ok(s),
            _ => refuse(Reason::NotAllowed("a pattern that is not a text".into()), e),
        }
    }

    fn case(
        &mut self,
        operand: &Option<Box<sql::Expr>>,
        conditions: &[sql::CaseWhen],
        else_result: &Option<Box<sql::Expr>>,
        d: usize,
    ) -> Result<Expr, ExprError> {
        let operand = operand
            .as_ref()
            .map(|o| self.expr(o, d).map(boxed))
            .transpose()?;
        let branches = conditions
            .iter()
            .map(|w| Ok((self.expr(&w.condition, d)?, self.expr(&w.result, d)?)))
            .collect::<Result<_, ExprError>>()?;
        let otherwise = else_result
            .as_ref()
            .map(|o| self.expr(o, d).map(boxed))
            .transpose()?;
        Ok(Expr::Case {
            operand,
            branches,
            otherwise,
        })
    }

    fn interval(&mut self, i: &sql::Interval, node: &sql::Expr) -> Result<Expr, ExprError> {
        let unit = match (
            &i.leading_field,
            &i.last_field,
            i.leading_precision,
            i.fractional_seconds_precision,
        ) {
            (Some(sql::DateTimeField::Year), None, None, None) => IntervalUnit::Year,
            (Some(sql::DateTimeField::Month), None, None, None) => IntervalUnit::Month,
            (Some(sql::DateTimeField::Day), None, None, None) => IntervalUnit::Day,
            _ => return refuse(Reason::BadInterval, node),
        };
        let amount = match i.value.as_ref() {
            sql::Expr::Value(v) => match &v.value {
                sql::Value::Number(n, false) | sql::Value::SingleQuotedString(n) => {
                    n.parse::<i64>().ok()
                }
                _ => None,
            },
            _ => None,
        };
        match amount {
            Some(amount) => Ok(Expr::Interval { amount, unit }),
            None => refuse(Reason::BadInterval, node),
        }
    }

    fn function(&mut self, f: &sql::Function, d: usize) -> Result<Expr, ExprError> {
        let plain = f.parameters == sql::FunctionArguments::None
            && f.filter.is_none()
            && f.null_treatment.is_none()
            && f.over.is_none()
            && f.within_group.is_empty();
        if !plain || f.name.0.len() != 1 {
            return refuse(Reason::NotAllowed(format!("function form {}", f.name)), f);
        }
        let name = f.name.to_string();
        let Some(function) = Function::from_name(&name) else {
            return refuse(Reason::UnknownFunction(name), f);
        };
        let args = match &f.args {
            sql::FunctionArguments::List(list)
                if list.duplicate_treatment.is_none() && list.clauses.is_empty() =>
            {
                list.args
                    .iter()
                    .map(|a| self.argument(a, d))
                    .collect::<Result<Vec<_>, _>>()?
            }
            sql::FunctionArguments::None => Vec::new(),
            _ => return refuse(Reason::NotAllowed(format!("arguments of {name}")), f),
        };
        let (min, max) = function.arity();
        if args.len() < min || args.len() > max {
            return refuse(
                Reason::Arity {
                    function,
                    got: args.len(),
                },
                f,
            );
        }
        let needs_text = matches!(
            function,
            Function::RegexpLike | Function::InList | Function::InReference
        );
        if needs_text && !matches!(args[1], Expr::Literal(Literal::Text(_))) {
            return refuse(Reason::TextRequired { function }, f);
        }
        Ok(Expr::Call { function, args })
    }

    fn argument(&mut self, a: &sql::FunctionArg, d: usize) -> Result<Expr, ExprError> {
        match a {
            sql::FunctionArg::Unnamed(sql::FunctionArgExpr::Expr(e)) => self.expr(e, d),
            other => refuse(Reason::NotAllowed(format!("argument {other}")), other),
        }
    }
}

/// Texts keep every character but control ones (NUL first: DuckDB accepts
/// it inside a literal, see decision 0033's verification).
fn clean(s: &str, node: &sql::Expr) -> Result<String, ExprError> {
    if s.chars().any(|c| c.is_control() && c != '\t') {
        return refuse(Reason::ControlCharacter, node);
    }
    Ok(s.to_string())
}

fn binary_op(op: &sql::BinaryOperator) -> Option<BinaryOp> {
    use sql::BinaryOperator as B;
    Some(match op {
        B::And => BinaryOp::And,
        B::Or => BinaryOp::Or,
        B::Eq => BinaryOp::Eq,
        B::NotEq => BinaryOp::NotEq,
        B::Lt => BinaryOp::Lt,
        B::LtEq => BinaryOp::LtEq,
        B::Gt => BinaryOp::Gt,
        B::GtEq => BinaryOp::GtEq,
        B::Plus => BinaryOp::Add,
        B::Minus => BinaryOp::Subtract,
        B::Multiply => BinaryOp::Multiply,
        B::Divide => BinaryOp::Divide,
        B::Modulo => BinaryOp::Modulo,
        B::StringConcat => BinaryOp::Concat,
        _ => return None,
    })
}

fn cast_type(t: &sql::DataType, node: &sql::Expr) -> Result<Type, ExprError> {
    use sql::DataType as D;
    Ok(match t {
        D::Text | D::Varchar(_) | D::String(_) => Type::Text,
        D::Int(_) | D::Integer(_) | D::BigInt(_) => Type::Integer,
        D::Decimal(_) | D::Numeric(_) | D::Double(_) | D::DoublePrecision => Type::Decimal,
        D::Date => Type::Date,
        D::Timestamp(_, _) => Type::Timestamp,
        D::Boolean | D::Bool => Type::Boolean,
        other => return refuse(Reason::UnsupportedType(other.to_string()), node),
    })
}

fn typed_string(ts: &sql::TypedString, node: &sql::Expr) -> Result<Expr, ExprError> {
    match (&ts.data_type, &ts.value.value) {
        (sql::DataType::Date, sql::Value::SingleQuotedString(s)) => {
            Ok(Expr::Literal(Literal::Date(clean(s, node)?)))
        }
        _ => refuse(
            Reason::NotAllowed(format!("typed text {}", ts.data_type)),
            node,
        ),
    }
}

/// A short name for a construct outside the grammar.
fn node_name(e: &sql::Expr) -> String {
    match e {
        sql::Expr::Subquery(_) | sql::Expr::Exists { .. } | sql::Expr::InSubquery { .. } => {
            "subquery".into()
        }
        sql::Expr::CompoundIdentifier(_) => "qualified name".into(),
        _ => {
            let text = e.to_string();
            let short: String = text.chars().take(40).collect();
            format!("expression {short}")
        }
    }
}
