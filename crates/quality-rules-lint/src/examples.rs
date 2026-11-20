//! A rule's examples run on the engine, not in a second evaluator (decision
//! 0033, point 7): each compiled rule with examples becomes one DuckDB
//! query over its example records; the caller runs it on an in-memory
//! DuckDB and [`check_examples`] compares what it gave with what each
//! example declares.

use std::collections::BTreeMap;

use quality_rules_compile::{Compiled, FieldType, Schema};
use quality_rules_model::{Position, Records, RulesFile, Text};
use quality_rules_sql::{quote_ident, quote_text, Engine, Tables};

use super::{Code, Finding};

/// What an example declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expected {
    Fails,
    Passes,
    Skips,
}

impl Expected {
    fn result(self) -> Option<bool> {
        match self {
            Self::Fails => Some(true),
            Self::Passes => Some(false),
            Self::Skips => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Fails => "quality.lint.example.fails",
            Self::Passes => "quality.lint.example.passes",
            Self::Skips => "quality.lint.example.skips",
        }
    }
}

/// One rule's examples as a query: one row per example, in order, with the
/// rule's result. Bind the cut `cut_params` times.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExampleQuery {
    pub rule: String,
    pub at: Position,
    pub sql: String,
    pub cut_params: usize,
    /// Per row: what it declares, and its place among those of its kind.
    pub expected: Vec<(Expected, usize)>,
}

type Record = BTreeMap<String, Option<Text>>;

pub(crate) fn queries(
    file: &RulesFile,
    schema: &Schema,
    compiled: &[Compiled],
) -> (Vec<ExampleQuery>, Vec<Finding>) {
    let mut queries = Vec::new();
    let mut findings = Vec::new();
    for c in compiled {
        let Some(rule) = super::rule_of(file, &c.id) else {
            continue;
        };
        let Some(examples) = &rule.examples else {
            continue;
        };
        let at = examples.at;
        let mut rows: Vec<(&Record, Expected, usize)> = Vec::new();
        for (records, kind) in [
            (&examples.value.fails, Expected::Fails),
            (&examples.value.passes, Expected::Passes),
            (&examples.value.skips, Expected::Skips),
        ] {
            rows.extend(
                applicable(records, c)
                    .into_iter()
                    .enumerate()
                    .map(|(i, r)| (r, kind, i + 1)),
            );
        }
        let unknown: Vec<&String> = rows
            .iter()
            .flat_map(|(r, ..)| r.keys())
            .filter(|k| !schema.fields.contains_key(*k))
            .collect();
        if let Some(name) = unknown.first() {
            findings.push(
                Finding::new(Code::UnknownField, Some(&c.id), Some(at))
                    .with("name", (*name).clone()),
            );
            continue;
        }
        if rows.is_empty() {
            continue;
        }
        match query(c, schema, &rows) {
            Ok((sql, cut_params)) => {
                let expected = rows.iter().map(|(_, kind, i)| (*kind, *i)).collect();
                queries.push(ExampleQuery {
                    rule: c.id.clone(),
                    at,
                    sql,
                    cut_params,
                    expected,
                });
            }
            Err(value) => findings.push(
                Finding::new(Code::BadValue, Some(&c.id), Some(at))
                    .with("parameter", "examples")
                    .with("value", value),
            ),
        }
    }
    (queries, findings)
}

/// A rule over several fields runs only the examples that name its field.
fn applicable<'a>(records: &'a Records, c: &Compiled) -> Vec<&'a Record> {
    records
        .all()
        .into_iter()
        .filter(|r| c.field.as_ref().is_none_or(|f| r.contains_key(f)))
        .collect()
}

/// `SELECT <result> FROM (VALUES …) AS t("__row", <every field>)`; a value
/// that does not fit its field's type is the error.
fn query(
    c: &Compiled,
    schema: &Schema,
    rows: &[(&Record, Expected, usize)],
) -> Result<(String, usize), String> {
    let rule =
        quality_rules_sql::rule(c, Engine::DuckDb, &Tables::new()).map_err(|e| format!("{e:?}"))?;
    let names: Vec<String> = schema
        .fields
        .keys()
        .map(|n| quote_ident(n).map_err(|e| format!("{e:?}")))
        .collect::<Result<_, _>>()?;
    let mut values = Vec::new();
    for (i, (record, ..)) in rows.iter().enumerate() {
        let cells: Vec<String> = schema
            .fields
            .iter()
            .map(|(name, t)| cell(*t, record.get(name).cloned().flatten()))
            .collect::<Result<_, _>>()?;
        values.push(format!("({i}, {})", cells.join(", ")));
    }
    let sql = format!(
        "SELECT {} FROM (VALUES {}) AS t(\"__row\", {}) ORDER BY \"__row\"",
        rule.text,
        values.join(", "),
        names.join(", ")
    );
    Ok((sql, rule.cut_params))
}

fn cell(t: FieldType, v: Option<Text>) -> Result<String, String> {
    let ty = match t {
        FieldType::Text | FieldType::Email => "VARCHAR",
        FieldType::Number => "DOUBLE",
        FieldType::Date => "DATE",
        FieldType::Boolean => "BOOLEAN",
    };
    let literal = match v {
        None => "NULL".to_string(),
        Some(Text(s)) => match t {
            FieldType::Text | FieldType::Email => quote_text(&s).map_err(|_| s.clone())?,
            FieldType::Number if s.trim().parse::<f64>().is_ok() => s.trim().to_string(),
            FieldType::Date if is_date(s.trim()) => format!("DATE '{}'", s.trim()),
            FieldType::Boolean if s.trim().parse::<bool>().is_ok() => s.trim().to_string(),
            _ => return Err(s),
        },
    };
    Ok(format!("CAST({literal} AS {ty})"))
}

fn is_date(d: &str) -> bool {
    let b = d.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

/// What the query gave against what each example declares.
pub fn check_examples(query: &ExampleQuery, results: &[Option<bool>]) -> Vec<Finding> {
    let got_key = |r: Option<bool>| match r {
        Some(true) => Expected::Fails.key(),
        Some(false) => Expected::Passes.key(),
        None => Expected::Skips.key(),
    };
    query
        .expected
        .iter()
        .zip(results.iter().copied().chain(std::iter::repeat(None)))
        .filter(|((kind, _), got)| kind.result() != *got)
        .map(|((kind, index), got)| {
            Finding::new(Code::Example, Some(&query.rule), Some(query.at))
                .with("expected_key", kind.key())
                .with("index", index.to_string())
                .with("got_key", got_key(got))
        })
        .collect()
}
