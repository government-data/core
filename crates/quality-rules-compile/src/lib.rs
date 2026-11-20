//! From a rules file to compiled rules (decision 0033): each rule's pattern
//! lowered to the neutral tree with the semantics stated once (point 4).
//! A compiled rule says where it applies, when its field's emptiness skips
//! it, and when it fails; [`Compiled::result`] joins them into the
//! three-valued result every engine computes.

mod context;
mod cross;
mod expand;
mod field;
mod params;
mod patterns;
mod text;

use std::collections::BTreeMap;

use quality_rules_model::Position;
use quality_rules_model::{Dimension, Pattern, RulesFile, Severity};
use quality_rules_tree::{BinaryOp, Expr, ExprError, Function, Literal};

/// The dataset's fields and their types, from its mapping (decision 0020).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Schema {
    pub fields: BTreeMap<String, FieldType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    Text,
    /// Text holding an e-mail address: the dataset's `email` fillers apply.
    Email,
    Number,
    Date,
    Boolean,
}

/// One rule, ready for an engine.
#[derive(Debug, Clone, PartialEq)]
pub struct Compiled {
    /// The rule's id; `R-12.Campo` for each field of a list, `R-30.1` for
    /// each value of `for_each`.
    pub id: String,
    pub field: Option<String>,
    pub pattern: Pattern,
    pub dimension: Dimension,
    pub severity: Severity,
    /// The record is in the rule's population and its condition holds.
    pub applies: Expr,
    /// The field is empty and the pattern leaves emptiness to `required`.
    pub skips: Option<Expr>,
    pub fails: Expr,
}

impl Compiled {
    /// `NULL` does not apply, `TRUE` fails, `FALSE` passes (decision 0019,
    /// point 4). A `NULL` anywhere else does not apply either (0033).
    pub fn result(&self) -> Expr {
        let coalesce = |e: &Expr| Expr::Call {
            function: Function::Coalesce,
            args: vec![e.clone(), Expr::Literal(Literal::Boolean(false))],
        };
        let null = || Expr::Literal(Literal::Null);
        let truth = |b| Expr::Literal(Literal::Boolean(b));
        let mut branches = vec![(Expr::Not(Box::new(coalesce(&self.applies))), null())];
        if let Some(skips) = &self.skips {
            branches.push((coalesce(skips), null()));
        }
        branches.push((self.fails.clone(), truth(true)));
        branches.push((Expr::Not(Box::new(self.fails.clone())), truth(false)));
        Expr::Case {
            operand: None,
            branches,
            otherwise: None,
        }
    }

    /// The dataset's fields the rule reads, each once: a derived field's are
    /// those it is computed from (decision 0043, point 2).
    pub fn reads(&self) -> Vec<String> {
        let result = self.result();
        let mut out: Vec<String> = Vec::new();
        for node in result.walk() {
            if let Expr::Field(name) = node {
                if !out.contains(name) {
                    out.push(name.clone());
                }
            }
        }
        out
    }
}

/// Something in a rule that keeps it from compiling.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    /// The rule's id as written.
    pub rule: String,
    /// Where in the YAML.
    pub at: Position,
    pub kind: ProblemKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProblemKind {
    UnknownField(String),
    UnknownPopulation(String),
    UnknownList(String),
    UnknownTable(String),
    UnknownReference(String),
    /// The pattern needs this parameter.
    Missing(&'static str),
    /// The pattern takes neither this parameter nor its alternative.
    NotApplicable(&'static str),
    /// A value that does not fit the field's type or the parameter.
    BadValue {
        parameter: &'static str,
        value: String,
    },
    /// An expression outside the grammar, with its place inside the text.
    Expression(ExprError),
    /// Allowed by the model, not compiled yet: an id (`table_on_several_fields`).
    NotSupported(&'static str),
}

/// Compiles every rule it can and reports the problems of the others.
pub fn compile(file: &RulesFile, schema: &Schema) -> (Vec<Compiled>, Vec<Problem>) {
    let mut ctx = context::Context::new(file, schema);
    let mut compiled = Vec::new();
    for rule in &file.rules {
        // An `extends` not merged with the global rules has no pattern yet.
        if rule.check.is_none() {
            ctx.report(&rule.id.value, rule.id.at, ProblemKind::Missing("check"));
            continue;
        }
        for unit in expand::expand(rule) {
            if let Some(c) = patterns::compile_one(&mut ctx, rule, &unit) {
                compiled.push(c);
            }
        }
    }
    (compiled, ctx.problems)
}

fn and(left: Expr, right: Expr) -> Expr {
    Expr::Binary {
        op: BinaryOp::And,
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn or(left: Expr, right: Expr) -> Expr {
    Expr::Binary {
        op: BinaryOp::Or,
        left: Box::new(left),
        right: Box::new(right),
    }
}
