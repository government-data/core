//! The neutral expression tree of the rule language (decision 0019,
//! point 3): what a rule's
//! conditions and checks become before any engine sees them. Only the
//! nodes below exist, so only they can reach SQL; each dialect writes them.

mod display;
mod parse;

pub use display::function_name;
pub use parse::{parse_expr, ExprError, Reason, MAX_DEPTH, MAX_NODES};

use quality_rules_model::NamedFormat;

/// An expression of the closed grammar.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A field of the dataset, by name (resolved against the mapping later).
    Field(String),
    Literal(Literal),
    /// The cut's date: dates compare with it, never with today.
    Cut,
    Not(Box<Expr>),
    Negate(Box<Expr>),
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    IsNull {
        expr: Box<Expr>,
        negated: bool,
    },
    InList {
        expr: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },
    Between {
        expr: Box<Expr>,
        low: Box<Expr>,
        high: Box<Expr>,
        negated: bool,
    },
    /// `LIKE` with a text pattern; the dialect adds its own `ESCAPE`.
    Like {
        expr: Box<Expr>,
        pattern: String,
        negated: bool,
    },
    Case {
        operand: Option<Box<Expr>>,
        branches: Vec<(Expr, Expr)>,
        otherwise: Option<Box<Expr>>,
    },
    Cast {
        expr: Box<Expr>,
        to: Type,
    },
    /// `INTERVAL 18 YEAR`: a whole number of years, months or days.
    Interval {
        amount: i64,
        unit: IntervalUnit,
    },
    Call {
        function: Function,
        args: Vec<Expr>,
    },
    // The nodes below come from patterns only: no rule text parses to them.
    /// The value has a format the platform maintains.
    Matches {
        expr: Box<Expr>,
        format: NamedFormat,
    },
    /// Letters and digits only, in upper case (`normalize: true`).
    Normalized(Box<Expr>),
    /// Another record has the same key (`unique`).
    Duplicated(Vec<Expr>),
}

/// A constant. Numbers keep their text, so no precision is lost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    Text(String),
    Number(String),
    Boolean(bool),
    Null,
    /// `DATE '2024-01-31'`.
    Date(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    And,
    Or,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    /// `||`
    Concat,
}

/// The types a `CAST` may name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Text,
    Integer,
    Decimal,
    Date,
    Timestamp,
    Boolean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntervalUnit {
    Year,
    Month,
    Day,
}

/// The closed list of functions: the platform's, which standard SQL lacks
/// or each engine spells differently, and a few standard ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Function {
    /// `NULL`, `''` or only spaces, the no-break space included.
    IsEmpty,
    /// A regular expression, anchored to the whole value.
    RegexpLike,
    /// A list declared by the dataset: `in_list(x, 'name')`.
    InList,
    /// A reference's column: `in_reference(x, 'advisors.CodigoAsesor')`.
    InReference,
    /// Days from a date to the cut.
    DaysSince,
    DaysBetween,
    YearsBetween,
    Upper,
    Lower,
    /// Spaces at both ends, the no-break space (U+00A0) counted as one.
    Trim,
    Length,
    Coalesce,
    Abs,
    Round,
}

impl Function {
    /// The name rules write, in lower case.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "is_empty" => Self::IsEmpty,
            "regexp_like" => Self::RegexpLike,
            "in_list" => Self::InList,
            "in_reference" => Self::InReference,
            "days_since" => Self::DaysSince,
            "days_between" => Self::DaysBetween,
            "years_between" => Self::YearsBetween,
            "upper" => Self::Upper,
            "lower" => Self::Lower,
            "trim" => Self::Trim,
            "length" => Self::Length,
            "coalesce" => Self::Coalesce,
            "abs" => Self::Abs,
            "round" => Self::Round,
            _ => return None,
        })
    }

    /// How many arguments it takes: at least, at most.
    pub fn arity(self) -> (usize, usize) {
        match self {
            Self::IsEmpty | Self::DaysSince | Self::Upper | Self::Lower => (1, 1),
            Self::Trim | Self::Length | Self::Abs => (1, 1),
            Self::RegexpLike | Self::InList | Self::InReference => (2, 2),
            Self::DaysBetween | Self::YearsBetween => (2, 2),
            Self::Round => (1, 2),
            Self::Coalesce => (2, usize::MAX),
        }
    }
}

impl Expr {
    /// The nodes directly under this one, in order.
    pub fn children(&self) -> Vec<&Expr> {
        match self {
            Self::Field(_) | Self::Literal(_) | Self::Cut | Self::Interval { .. } => Vec::new(),
            Self::Not(x) | Self::Negate(x) | Self::Normalized(x) => vec![x],
            Self::Binary { left, right, .. } => vec![left, right],
            Self::IsNull { expr, .. }
            | Self::Like { expr, .. }
            | Self::Cast { expr, .. }
            | Self::Matches { expr, .. } => {
                vec![expr]
            }
            Self::InList { expr, list, .. } => std::iter::once(&**expr).chain(list).collect(),
            Self::Between {
                expr, low, high, ..
            } => vec![expr, low, high],
            Self::Case {
                operand,
                branches,
                otherwise,
            } => operand
                .as_deref()
                .into_iter()
                .chain(branches.iter().flat_map(|(w, t)| [w, t]))
                .chain(otherwise.as_deref())
                .collect(),
            Self::Call { args, .. } | Self::Duplicated(args) => args.iter().collect(),
        }
    }

    /// This node and every node under it.
    pub fn walk(&self) -> Vec<&Expr> {
        let mut out = vec![self];
        let mut i = 0;
        while i < out.len() {
            let children = out[i].children();
            out.extend(children);
            i += 1;
        }
        out
    }
}
