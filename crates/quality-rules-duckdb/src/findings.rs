//! The findings of a cut and its remediation queues (decision 0059), over
//! the records' ICD (`__icd`, decision 0057): each record failing a rule,
//! known by the keyed hash of its key, with the guide's priority (formula
//! 5) and its group in each dimension; the cut it opened at, carried from
//! the previous cut's findings; and each queue's open and resolved
//! findings by that cut. The record's hash is computed in the engine, the
//! key's pads bound in a table, never in the SQL's text.

use quality::mapping::{FieldRole, Mapping};
use quality::worker_api::{QueueFindings, SfcCell};

use crate::icd::{measured, role_field, IcdRule, Measured};
use crate::{invalid, quote_text, Error, Runner};
use quality_rules_sql::double;

/// The most queues kept per dimension, those with the most open findings.
const MOST_QUEUES: usize = 1000;

/// HMAC-SHA-256's block, in bytes.
const BLOCK: usize = 64;

/// The previous cut's findings, as relations of the same columns as
/// `__findings` (`record`, `rule`, `since`) and `__finding_groups`
/// (`record`, `dimension`, `grp`), only that cut's rows.
#[derive(Debug, Clone, Copy)]
pub struct Previous<'a> {
    pub findings: &'a str,
    pub groups: &'a str,
}

/// Closeness to the SFC threshold (the guide's formula 5).
fn prox(p: f64) -> f64 {
    if p >= 0.92 {
        0.2
    } else if p >= 0.90 {
        0.5
    } else if p >= 0.80 {
        1.0
    } else if p >= 0.70 {
        0.6
    } else {
        0.3
    }
}

/// The key's inner and outer pads; a key longer than a block is refused,
/// as the tenant's is 64 characters.
fn pads(key: &str) -> Result<(Vec<u8>, Vec<u8>), Error> {
    let bytes = key.as_bytes();
    if bytes.len() > BLOCK {
        return Err(invalid("an entity key longer than a block"));
    }
    let mut block = [0u8; BLOCK];
    block[..bytes.len()].copy_from_slice(bytes);
    let pad = |x: u8| block.iter().map(|b| b ^ x).collect();
    Ok((pad(0x36), pad(0x5c)))
}

/// Each measured field's term of the priority (`f{n}` its ICD): its weight
/// over 4, what its ICD lacks, and its closeness to the SFC threshold from
/// its cell's share right among the active records, as the SFC view reads
/// it (decision 0058); 0.2, the lowest, for a field with no cell.
fn terms(fields: &[Measured], cells: &[SfcCell]) -> Vec<String> {
    fields
        .iter()
        .enumerate()
        .map(|(n, f)| {
            let closeness = cells
                .iter()
                .find(|c| c.field == f.name && c.total > 0)
                .map_or(0.2, |c| prox(c.ok as f64 / c.total as f64));
            let c = double(f64::from(f.weight) / 4.0);
            let closeness = double(closeness);
            format!("COALESCE({c} * (DOUBLE '1' - \"f{n}\") * {closeness}, DOUBLE '0')")
        })
        .collect()
}

impl Runner {
    /// The findings of the cut whose ICD [`Runner::icd`] just computed,
    /// into `__findings` and `__finding_groups`, and each queue's counts;
    /// `__icd` is dropped. None without a record key or an ICD.
    pub fn findings(
        &self,
        mapping: &Mapping,
        rules: &[IcdRule],
        cut: &str,
        key: &str,
        cells: &[SfcCell],
        previous: Option<Previous<'_>>,
    ) -> Result<Vec<QueueFindings>, Error> {
        let present: i64 = self.con.query_row(
            "SELECT count(*) FROM duckdb_tables() WHERE temporary AND table_name = '__icd'",
            [],
            |r| r.get(0),
        )?;
        if present == 0 || crate::keys::record_key(mapping)?.is_none() {
            return Ok(Vec::new());
        }
        self.hashed(key)?;
        let out = self
            .found(mapping, rules, cut, cells, previous)
            .and_then(|()| self.queues(mapping, previous));
        self.con
            .execute_batch("DROP TABLE IF EXISTS \"__icd\"; DROP TABLE IF EXISTS \"__icdh\";")?;
        out
    }

    /// `__icd` with each keyed record's hash, as `__icdh`.
    fn hashed(&self, key: &str) -> Result<(), Error> {
        let (inner, outer) = pads(key)?;
        self.con.execute_batch(
            "CREATE OR REPLACE TEMP TABLE \"__hmac\" (inner_pad BLOB, outer_pad BLOB)",
        )?;
        self.con.execute(
            "INSERT INTO \"__hmac\" VALUES (?, ?)",
            duckdb::params![inner, outer],
        )?;
        let made = self.con.execute_batch(
            "CREATE OR REPLACE TEMP TABLE \"__icdh\" AS
             SELECT sha256(h.outer_pad || unhex(sha256(h.inner_pad || encode(i.\"__r\"))))
                        AS \"__h\", i.*
               FROM \"__icd\" i, \"__hmac\" h WHERE i.\"__r\" IS NOT NULL",
        );
        self.con.execute_batch("DROP TABLE \"__hmac\"")?;
        made
    }

    /// Each failing (record, rule) with its priority and since, and each
    /// finding's record's group in each dimension.
    fn found(
        &self,
        mapping: &Mapping,
        rules: &[IcdRule],
        cut: &str,
        cells: &[SfcCell],
        previous: Option<Previous<'_>>,
    ) -> Result<(), Error> {
        let timely = role_field(mapping, FieldRole::Updated)?.is_some();
        let fields = measured(mapping, rules, timely);
        let terms = terms(&fields, cells);
        let mut each = Vec::new();
        for (i, rule) in rules.iter().enumerate() {
            let own: Vec<&String> = fields
                .iter()
                .enumerate()
                .filter(|(_, f)| rule.reads.contains(&f.name))
                .map(|(n, _)| &terms[n])
                .collect();
            let priority = match own.len() {
                0 => "DOUBLE '0'".to_string(),
                1 => own[0].clone(),
                _ => format!(
                    "greatest({})",
                    own.iter()
                        .map(|t| t.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            each.push(format!(
                "SELECT \"__h\" AS record, {} AS rule, {priority} AS priority
                   FROM \"__icdh\" WHERE \"r{i}\"",
                quote_text(&rule.id).map_err(invalid)?
            ));
        }
        if each.is_empty() {
            each.push(
                "SELECT CAST(NULL AS VARCHAR) AS record, CAST(NULL AS VARCHAR) AS rule, \
                 CAST(NULL AS DOUBLE) AS priority WHERE FALSE"
                    .into(),
            );
        }
        let cut = quote_text(cut).map_err(invalid)?;
        let (join, since) = match previous {
            Some(p) => (
                format!("LEFT JOIN {} p USING (record, rule)", p.findings),
                format!("COALESCE(p.since, CAST({cut} AS DATE))"),
            ),
            None => (String::new(), format!("CAST({cut} AS DATE)")),
        };
        self.con.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE \"__findings\" AS
             WITH f AS (SELECT record, rule, max(priority) AS priority
                          FROM ({}) GROUP BY record, rule)
             SELECT f.record, f.rule, f.priority, {since} AS since FROM f {join}",
            each.join(" UNION ALL ")
        ))?;
        let mut groups = vec![
            "SELECT CAST(NULL AS VARCHAR) AS record, CAST(NULL AS VARCHAR) AS dimension, \
             CAST(NULL AS VARCHAR) AS grp WHERE FALSE"
                .to_string(),
        ];
        for (j, name) in dimensions(mapping).enumerate() {
            groups.push(format!(
                "SELECT \"__h\", {}, min(\"__g{j}\") FROM \"__icdh\"
                  WHERE \"__h\" IN (SELECT record FROM \"__findings\") GROUP BY \"__h\"",
                quote_text(name).map_err(invalid)?
            ));
        }
        self.con.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE \"__finding_groups\" AS {}",
            groups.join(" UNION ALL ")
        ))
    }

    /// Each queue's open findings by since, and those of the previous cut
    /// it no longer has, by the group they had; the queues with the most
    /// open findings of each dimension.
    fn queues(
        &self,
        mapping: &Mapping,
        previous: Option<Previous<'_>>,
    ) -> Result<Vec<QueueFindings>, Error> {
        let names: Vec<String> = dimensions(mapping)
            .map(|d| quote_text(d).map_err(invalid))
            .collect::<Result<_, _>>()?;
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let gone = match previous {
            Some(p) => format!(
                "UNION ALL
                 SELECT g.dimension, g.grp, p.since, 0, count(*), DOUBLE '0'
                   FROM {} p JOIN {} g USING (record)
                  WHERE g.dimension IN ({}) AND NOT EXISTS
                        (SELECT 1 FROM \"__findings\" f WHERE f.record = p.record AND f.rule = p.rule)
                  GROUP BY g.dimension, g.grp, p.since",
                p.findings,
                p.groups,
                names.join(", ")
            ),
            None => String::new(),
        };
        let sql = format!(
            "WITH u (dimension, grp, since, open, resolved, priority) AS (
                 SELECT g.dimension, g.grp, f.since, count(*), 0, sum(f.priority)
                   FROM \"__findings\" f JOIN \"__finding_groups\" g USING (record)
                  GROUP BY g.dimension, g.grp, f.since
                 {gone}),
                  q AS (SELECT dimension, grp, since, sum(open) AS open,
                               sum(resolved) AS resolved, sum(priority) AS priority
                          FROM u GROUP BY dimension, grp, since),
                  t AS (SELECT *, sum(open) OVER (PARTITION BY dimension, grp) AS total FROM q),
                  r AS (SELECT *, dense_rank() OVER (PARTITION BY dimension
                                   ORDER BY total DESC NULLS LAST, grp ASC NULLS LAST) AS n
                          FROM t)
             SELECT dimension, grp, CAST(since AS VARCHAR), CAST(open AS BIGINT),
                    CAST(resolved AS BIGINT), CAST(priority AS DOUBLE)
               FROM r WHERE n <= {MOST_QUEUES}
              ORDER BY dimension ASC NULLS LAST, n ASC NULLS LAST, since ASC NULLS LAST"
        );
        let mut stmt = self.con.prepare(&sql)?;
        let rows = stmt.query_map([], |r| {
            Ok(QueueFindings {
                dimension: r.get(0)?,
                group: r.get(1)?,
                since: r.get(2)?,
                open: r.get(3)?,
                resolved: r.get(4)?,
                priority: r.get(5)?,
            })
        })?;
        rows.collect()
    }
}

/// The fields holding the `dimension` role, in the mapping's order.
fn dimensions(mapping: &Mapping) -> impl Iterator<Item = &String> {
    mapping
        .fields
        .iter()
        .filter(|(_, f)| f.value.roles.contains(&FieldRole::Dimension))
        .map(|(name, _)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closeness_is_the_guides() {
        let seen: Vec<f64> = [0.95, 0.92, 0.91, 0.85, 0.80, 0.75, 0.5]
            .into_iter()
            .map(prox)
            .collect();
        assert_eq!(seen, [0.2, 0.2, 0.5, 1.0, 1.0, 0.6, 0.3]);
        assert!(pads(&"k".repeat(65)).is_err());
    }
}
