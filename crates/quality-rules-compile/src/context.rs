//! What compiling a rule needs from its file and its dataset: fields and
//! their types, derived fields, lists, and the problems found so far.

use std::collections::BTreeMap;

use super::expand::Unit;
use super::text::{normalize_comparisons, rewrite};
use super::{FieldType, Problem, ProblemKind, Schema};
use quality_rules_model::{At, Position};
use quality_rules_model::{Case, Rule, RulesFile, Text};
use quality_rules_tree::{parse_expr, BinaryOp, Expr, Function, Literal, Type};

pub(super) struct Context<'a> {
    pub(super) file: &'a RulesFile,
    schema: &'a Schema,
    derived: BTreeMap<String, (Expr, FieldType)>,
    pub(super) problems: Vec<Problem>,
}

impl<'a> Context<'a> {
    pub(super) fn new(file: &'a RulesFile, schema: &'a Schema) -> Self {
        let mut ctx = Self {
            file,
            schema,
            derived: BTreeMap::new(),
            problems: Vec::new(),
        };
        for (name, text) in &file.derived {
            match parse_expr(&text.value) {
                Ok(e) => {
                    // A derived field reads the dataset's fields only.
                    let unknown: Vec<String> = e
                        .walk()
                        .into_iter()
                        .filter_map(|n| match n {
                            Expr::Field(f) if !schema.fields.contains_key(f) => Some(f.clone()),
                            _ => None,
                        })
                        .collect();
                    for field in &unknown {
                        let kind = ProblemKind::UnknownField(field.clone());
                        ctx.report(&format!("derived.{name}"), text.at, kind);
                    }
                    if unknown.is_empty() {
                        let kind = derived_type(&e, schema);
                        ctx.derived.insert(name.clone(), (e, kind));
                    }
                }
                Err(e) => ctx.report(
                    &format!("derived.{name}"),
                    text.at,
                    ProblemKind::Expression(e),
                ),
            }
        }
        for (name, reference) in &file.references {
            if let Some(filter) = &reference.filter {
                if let Err(e) = parse_expr(&filter.value) {
                    let kind = ProblemKind::Expression(e);
                    ctx.report(&format!("references.{name}"), filter.at, kind);
                }
            }
        }
        ctx
    }

    pub(super) fn report(&mut self, rule: &str, at: Position, kind: ProblemKind) {
        self.problems.push(Problem {
            rule: rule.to_string(),
            at,
            kind,
        });
    }

    /// The type of a field of the dataset or of a derived field.
    pub(super) fn type_of(&self, name: &str) -> Option<FieldType> {
        self.schema
            .fields
            .get(name)
            .copied()
            .or_else(|| self.derived.get(name).map(|d| d.1))
    }

    /// A field as an expression: itself, or its derivation.
    pub(super) fn field(
        &mut self,
        rule: &Rule,
        name: &str,
        at: Position,
    ) -> Option<(Expr, FieldType)> {
        if let Some((e, t)) = self.derived.get(name) {
            return Some((e.clone(), *t));
        }
        match self.schema.fields.get(name) {
            Some(t) => Some((Expr::Field(name.to_string()), *t)),
            None => {
                self.report(
                    &rule.id.value,
                    at,
                    ProblemKind::UnknownField(name.to_string()),
                );
                None
            }
        }
    }

    pub(super) fn list(&mut self, rule: &Rule, name: &str, at: Position) -> Option<Vec<Text>> {
        let found = self.file.lists.get(name).map(|l| l.value.clone());
        if found.is_none() {
            self.report(
                &rule.id.value,
                at,
                ProblemKind::UnknownList(name.to_string()),
            );
        }
        found
    }

    /// A text of the rule as an expression: parsed, `for_each` values in,
    /// derived fields and lists expanded, fields checked, texts compared as
    /// decision 0033 says.
    pub(super) fn expr(&mut self, rule: &Rule, unit: &Unit, text: &At<String>) -> Option<Expr> {
        let parsed = match parse_expr(&unit.subst(&text.value)) {
            Ok(e) => e,
            Err(e) => {
                self.report(&rule.id.value, text.at, ProblemKind::Expression(e));
                return None;
            }
        };
        let mut ok = true;
        let expanded = rewrite(parsed, &mut |e| match e {
            Expr::Field(name) => match self.field(rule, &name, text.at) {
                Some((e, _)) => e,
                None => {
                    ok = false;
                    Expr::Field(name)
                }
            },
            Expr::Call {
                function: Function::InList,
                args,
            } => self.in_list(rule, args, text.at, &mut ok),
            Expr::Call {
                function: Function::InReference,
                args,
            } => {
                let declared = match args.get(1) {
                    Some(Expr::Literal(Literal::Text(r))) => {
                        let name = r.split('.').next().unwrap_or_default();
                        r.contains('.') && self.file.references.contains_key(name)
                    }
                    _ => false,
                };
                if !declared {
                    let shown = args.get(1).map(ToString::to_string).unwrap_or_default();
                    let kind = ProblemKind::UnknownReference(shown.trim_matches('\'').to_string());
                    self.report(&rule.id.value, text.at, kind);
                    ok = false;
                }
                Expr::Call {
                    function: Function::InReference,
                    args,
                }
            }
            other => other,
        });
        ok.then(|| self.normalized(rule, expanded))
    }

    /// Text comparisons trimmed and case-insensitive unless `case: exact`.
    pub(super) fn normalized(&self, rule: &Rule, e: Expr) -> Expr {
        match rule.case.as_ref().map(|c| c.value) {
            Some(Case::Exact) => e,
            _ => normalize_comparisons(e, &|name| self.type_of(name)),
        }
    }

    /// `in_list(x, 'name')` becomes `x IN (…)` with the list's values.
    fn in_list(&mut self, rule: &Rule, mut args: Vec<Expr>, at: Position, ok: &mut bool) -> Expr {
        let name = match args.pop() {
            Some(Expr::Literal(Literal::Text(name))) => name,
            _ => unreachable!("the parser checks in_list's second argument"),
        };
        let expr = Box::new(args.pop().unwrap_or(Expr::Literal(Literal::Null)));
        match self.list(rule, &name, at) {
            Some(values) => {
                let list = values
                    .into_iter()
                    .map(|v| Expr::Literal(Literal::Text(v.0)))
                    .collect();
                Expr::InList {
                    expr,
                    list,
                    negated: false,
                }
            }
            None => {
                *ok = false;
                Expr::Literal(Literal::Null)
            }
        }
    }
}

/// What a derived field holds, from its outermost node.
fn derived_type(e: &Expr, schema: &Schema) -> FieldType {
    match e {
        // A CASE is what its branches give.
        Expr::Case { branches, .. } if !branches.is_empty() => derived_type(&branches[0].1, schema),
        Expr::Field(f) => schema.fields.get(f).copied().unwrap_or(FieldType::Text),
        Expr::Cast { to, .. } => match to {
            Type::Text => FieldType::Text,
            Type::Integer | Type::Decimal => FieldType::Number,
            Type::Date | Type::Timestamp => FieldType::Date,
            Type::Boolean => FieldType::Boolean,
        },
        Expr::Call {
            function: Function::DaysBetween | Function::YearsBetween | Function::DaysSince,
            ..
        } => FieldType::Number,
        Expr::Call {
            function: Function::Length | Function::Abs | Function::Round,
            ..
        } => FieldType::Number,
        Expr::Binary {
            op: BinaryOp::Add | BinaryOp::Subtract,
            right,
            ..
        } if matches!(**right, Expr::Interval { .. }) => FieldType::Date,
        Expr::Binary {
            op: BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide,
            ..
        } => FieldType::Number,
        Expr::Literal(Literal::Date(_)) | Expr::Cut => FieldType::Date,
        Expr::Literal(Literal::Number(_)) => FieldType::Number,
        Expr::Binary { .. }
        | Expr::IsNull { .. }
        | Expr::InList { .. }
        | Expr::Between { .. }
        | Expr::Like { .. } => FieldType::Boolean,
        _ => FieldType::Text,
    }
}
