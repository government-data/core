//! A rule's failures grouped by the shape of what it read (decision 0043,
//! point 3): each digit read as `9` and each letter as `A`, the rest kept,
//! so `+57 300-123` is `+99 999-999`; over every failure of the cut, the
//! largest groups first, each with how many and a few of its records. A
//! shape holds no letter nor digit of a value; the records do, and the
//! caller masks and audits them as any other.

use quality_rules_compile::Compiled;
use quality_rules_sql::{Engine, Tables};

use crate::sample::{fields_read, Shown};
use crate::{invalid, quoted, Error, Runner};

/// The longest shape kept: longer ones are cut.
const SHAPE_LEN: usize = 40;

/// Failures of one shape: how many, and a few, each its values by field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailGroup {
    /// The shape of the fields the rule reads, `·` between them; `∅` for
    /// an empty one.
    pub shape: String,
    pub count: u64,
    pub records: Vec<Vec<(String, Option<String>)>>,
}

/// The shape of `field`'s value, as SQL.
fn shape_of(field: &str) -> Result<String, Error> {
    let v = format!("CAST({} AS VARCHAR)", quoted(field)?);
    Ok(format!(
        "coalesce(substr(regexp_replace(regexp_replace({v}, '\\p{{N}}', '9', 'g'), '\\p{{L}}', 'A', 'g'), 1, {SHAPE_LEN}), '∅')"
    ))
}

impl Runner {
    /// The `top` largest groups of `rule`'s failures over `dataset` at
    /// `cut`, by shape, each with up to `each` records (the key's fields,
    /// those the rule reads and the extra ones of `shown`), by the key.
    pub fn fail_groups(
        &self,
        dataset: &str,
        rule: &Compiled,
        tables: &Tables,
        shown: Shown<'_>,
        cut: &str,
        (top, each): (usize, usize),
    ) -> Result<Vec<FailGroup>, Error> {
        let read = fields_read(rule);
        let mut fields: Vec<String> = shown.key.to_vec();
        for f in read.iter().chain(shown.extra.iter()) {
            if !fields.contains(f) {
                fields.push(f.clone());
            }
        }
        let shape = if read.is_empty() {
            "'∅'".to_string()
        } else {
            read.iter()
                .map(|f| shape_of(f))
                .collect::<Result<Vec<_>, Error>>()?
                .join(" || ' · ' || ")
        };
        let sql = quality_rules_sql::rule(rule, Engine::DuckDb, tables)
            .map_err(|e| invalid(format!("{}: {e:?}", rule.id)))?;
        let columns = fields
            .iter()
            .enumerate()
            .map(|(i, f)| Ok(format!("CAST({} AS VARCHAR) AS \"c{i}\"", quoted(f)?)))
            .collect::<Result<Vec<_>, Error>>()?;
        let names: Vec<String> = (0..fields.len()).map(|i| format!("k.\"c{i}\"")).collect();
        let order = (0..shown.key.len().max(1).min(fields.len()))
            .map(|i| format!("\"c{i}\" ASC NULLS LAST"))
            .collect::<Vec<_>>();
        let order = if order.is_empty() {
            "1".to_string()
        } else {
            order.join(", ")
        };
        let query = format!(
            "WITH r AS (SELECT {cols}, {shape} AS shape, ({rule}) AS outcome FROM {ds}),
                  f AS (SELECT * FROM r WHERE outcome),
                  g AS (SELECT shape, count(*) AS n FROM f GROUP BY shape
                         ORDER BY n DESC NULLS LAST, shape ASC NULLS LAST LIMIT {top}),
                  k AS (SELECT f.*, row_number() OVER (PARTITION BY f.shape ORDER BY {order}) AS i
                          FROM f JOIN g USING (shape))
             SELECT g.shape, g.n, {names} FROM g LEFT JOIN k ON k.shape = g.shape AND k.i <= {each}
             ORDER BY g.n DESC NULLS LAST, g.shape ASC NULLS LAST, k.i ASC NULLS LAST",
            cols = columns.join(", "),
            rule = sql.text,
            ds = quoted(dataset)?,
            names = names.join(", "),
        );
        let mut stmt = self.con.prepare(&query)?;
        let cuts = vec![cut; sql.cut_params];
        let mut groups: Vec<FailGroup> = Vec::new();
        let mut rows = stmt.query(duckdb::params_from_iter(cuts.iter()))?;
        while let Some(r) = rows.next()? {
            let shape: String = r.get(0)?;
            let count = u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0);
            let values = (0..fields.len())
                .map(|i| Ok((fields[i].clone(), r.get::<_, Option<String>>(i + 2)?)))
                .collect::<Result<Vec<_>, Error>>()?;
            // A group's first row; a group with no record left has none.
            if groups.last().is_none_or(|g| g.shape != shape) {
                groups.push(FailGroup {
                    shape,
                    count,
                    records: Vec::new(),
                });
            }
            if values.iter().any(|(_, v)| v.is_some()) {
                if let Some(g) = groups.last_mut() {
                    g.records.push(values);
                }
            }
        }
        Ok(groups)
    }
}
