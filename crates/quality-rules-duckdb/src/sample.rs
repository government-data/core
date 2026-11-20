//! A rule's read of records (decision 0043, point 3): a few records of
//! each outcome, fails, passes and does not apply, each with its key, the
//! fields the rule reads and any other asked for, in a fixed order (by the
//! key), so the same
//! test reads the same records. Values come out here, as text: the caller
//! masks them for whoever may not see them and audits every read.

use quality_rules_compile::Compiled;
use quality_rules_sql::{Engine, Tables};

use crate::{invalid, quoted, Error, Runner};

/// What a rule did with a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Fails,
    Passes,
    /// The rule does not apply to it (`NULL`).
    Skips,
}

/// A record a rule was run on: its outcome, and its values by field (the
/// key's first, then the fields the rule reads), `None` where empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sampled {
    pub outcome: Outcome,
    pub values: Vec<(String, Option<String>)>,
}

/// The fields a read shows: the key's, which order it, then any other
/// asked for besides those the rule reads.
#[derive(Debug, Clone, Copy)]
pub struct Shown<'a> {
    pub key: &'a [String],
    pub extra: &'a [String],
}

/// The fields a rule reads, each once.
pub fn fields_read(rule: &Compiled) -> Vec<String> {
    rule.reads()
}

impl Runner {
    /// Up to `each` records of every outcome of `rule` over `dataset` at
    /// `cut`, with the key's fields, those the rule reads, and the extra
    /// ones of `shown`.
    pub fn sample(
        &self,
        dataset: &str,
        rule: &Compiled,
        tables: &Tables,
        shown: Shown<'_>,
        cut: &str,
        each: usize,
    ) -> Result<Vec<Sampled>, Error> {
        let key = shown.key;
        let mut fields: Vec<String> = key.to_vec();
        for f in fields_read(rule)
            .into_iter()
            .chain(shown.extra.iter().cloned())
        {
            if !fields.contains(&f) {
                fields.push(f);
            }
        }
        let sql = quality_rules_sql::rule(rule, Engine::DuckDb, tables)
            .map_err(|e| invalid(format!("{}: {e:?}", rule.id)))?;
        let columns = fields
            .iter()
            .enumerate()
            .map(|(i, f)| Ok(format!("CAST({} AS VARCHAR) AS \"c{i}\"", quoted(f)?)))
            .collect::<Result<Vec<_>, Error>>()?;
        let order: Vec<String> = (0..key.len().max(1).min(fields.len()))
            .map(|i| format!("\"c{i}\""))
            .collect();
        let order = if order.is_empty() {
            "1".to_string()
        } else {
            order.join(", ")
        };
        let query = format!(
            "WITH r AS (SELECT {}{}({}) AS outcome FROM {}),
                  n AS (SELECT *, row_number() OVER (PARTITION BY outcome ORDER BY {order}) AS k FROM r)
             SELECT outcome, {} FROM n WHERE k <= {each}
             ORDER BY outcome DESC NULLS LAST, k",
            columns.join(", "),
            if columns.is_empty() { "" } else { ", " },
            sql.text,
            quoted(dataset)?,
            (0..fields.len()).map(|i| format!("\"c{i}\"")).collect::<Vec<_>>().join(", "),
        );
        let mut stmt = self.con.prepare(&query)?;
        let cuts = vec![cut; sql.cut_params];
        let rows = stmt.query_map(duckdb::params_from_iter(cuts.iter()), |r| {
            let outcome = match r.get::<_, Option<bool>>(0)? {
                Some(true) => Outcome::Fails,
                Some(false) => Outcome::Passes,
                None => Outcome::Skips,
            };
            let values = (0..fields.len())
                .map(|i| Ok((fields[i].clone(), r.get::<_, Option<String>>(i + 1)?)))
                .collect::<Result<Vec<_>, Error>>()?;
            Ok(Sampled { outcome, values })
        })?;
        rows.collect()
    }
}
