//! The ICD within a dataset (decision 0057, the guide's formulas): per
//! field and record the components C (its own rules), S (rules across
//! fields) and A (timeliness, for fields that expire), weighted and
//! renormalized over those that apply; per record the fields' ICD weighed
//! by priority; per entity the mean of its records' per field, the worst
//! for critical fields. Over each rule's result as `quality-rules` compiled
//! it: never a second evaluator.

use quality::mapping::{FieldRole, Mapping};
use quality::worker_api::{GroupMetrics, SfcCell};
use quality_rules_compile::Compiled;
use quality_rules_model::Severity;
use quality_rules_sql::{Engine, Sql, Tables};

use crate::groups::{dimensions, serious};
use crate::keys::{entity_key, record_key};
use crate::sfc::Cells;
use crate::{invalid, quote_text, quoted, Error, Runner};

/// The components' weights: C, S and A.
const W_C: f64 = 0.40;
const W_S: f64 = 0.15;
const W_A: f64 = 0.15;
/// A critical field's weight: its entity's ICD takes its worst record.
const CRITICAL: u8 = 4;

/// One rule as the ICD reads it: its SQL, its weight and its fields.
#[derive(Debug, Clone)]
pub struct IcdRule {
    /// The rule's id, as its findings name it (decision 0059).
    pub id: String,
    pub sql: Sql,
    pub severity: Severity,
    /// The fields it reads: one, it counts in C; more, in S of each.
    pub reads: Vec<String>,
}

impl IcdRule {
    pub fn of(rule: &Compiled, tables: &Tables) -> Result<Self, Error> {
        let sql = quality_rules_sql::rule(rule, Engine::DuckDb, tables)
            .map_err(|e| invalid(format!("{}: {e:?}", rule.id)))?;
        Ok(Self {
            id: rule.id.clone(),
            sql,
            severity: rule.severity,
            reads: rule.reads(),
        })
    }
}

fn severity(s: Severity) -> f64 {
    match s {
        Severity::Blocking => 1.0,
        Severity::Major => 0.6,
        Severity::Minor => 0.3,
    }
}

/// An entity's ICD in the dataset, by its normalized key (decision 0022).
#[derive(Debug, Clone, PartialEq)]
pub struct EntityIcd {
    pub key: String,
    pub icd: Option<f64>,
    pub segment: Option<String>,
}

/// What an evaluation's ICD gives.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct IcdReport {
    /// The mean over entities, or over records without an entity.
    pub igcd: Option<f64>,
    /// Each field's mean ICD over the records where it has one.
    pub fields: Vec<(String, Option<f64>)>,
    pub entities: Vec<EntityIcd>,
    /// Each group of each field holding the `dimension` role.
    pub groups: Vec<GroupMetrics>,
    /// Each SFC cell, over the same records (decision 0058).
    pub cells: Vec<SfcCell>,
}

/// A field with a component: its weight and the SQL of each one; the
/// `n`th measured is the column `f{n}` of `__icd`.
pub(crate) struct Measured {
    pub name: String,
    pub weight: u8,
    parts: Vec<(f64, String)>,
}

/// What one evaluation's ICD reads: the SQL of the entity's key, the
/// update date's and the segment's fields, quoted.
struct Plan<'a> {
    dataset: &'a str,
    rules: &'a [IcdRule],
    cut: &'a str,
    fields: &'a [Measured],
    key: Option<String>,
    /// The record's own key (decision 0059).
    record: Option<String>,
    updated: Option<String>,
    segment: Option<String>,
    /// The fields holding the `dimension` role, quoted.
    dimensions: Vec<String>,
    cells: &'a Cells,
}

/// `1 - failed weight / applying weight` over `rules` (index, severity).
fn conformity(rules: &[(usize, f64)]) -> Option<String> {
    if rules.is_empty() {
        return None;
    }
    let sum = |term: &dyn Fn(usize) -> String| {
        rules
            .iter()
            .map(|(i, s)| format!("{s} * {}", term(*i)))
            .collect::<Vec<_>>()
            .join(" + ")
    };
    Some(format!(
        "1 - ({}) / NULLIF({}, 0)",
        sum(&|i| format!("COALESCE(\"r{i}\"::INT, 0)")),
        sum(&|i| format!("(\"r{i}\" IS NOT NULL)::INT"))
    ))
}

/// `Σ w·x / Σ w·[x applies]`: `NULL` when none applies.
fn weighted(parts: &[(f64, String)]) -> String {
    if parts.is_empty() {
        return "NULL".to_string();
    }
    let (top, bottom): (Vec<_>, Vec<_>) = parts
        .iter()
        .map(|(w, x)| {
            (
                format!("{w} * COALESCE({x}, 0)"),
                format!("{w} * ({x} IS NOT NULL)::INT"),
            )
        })
        .unzip();
    format!("({}) / NULLIF({}, 0)", top.join(" + "), bottom.join(" + "))
}

pub(crate) fn measured(mapping: &Mapping, rules: &[IcdRule], timely: bool) -> Vec<Measured> {
    let mut out = Vec::new();
    for (name, f) in &mapping.fields {
        let (mut c, mut s) = (Vec::new(), Vec::new());
        for (i, r) in rules.iter().enumerate() {
            if r.reads.iter().any(|x| x == name) {
                let to = if r.reads.len() == 1 { &mut c } else { &mut s };
                to.push((i, severity(r.severity)));
            }
        }
        let mut parts = Vec::new();
        parts.extend(conformity(&c).map(|x| (W_C, format!("({x})"))));
        parts.extend(conformity(&s).map(|x| (W_S, format!("({x})"))));
        if timely && f.value.expires {
            parts.push((W_A, "\"__a\"".to_string()));
        }
        if !parts.is_empty() {
            out.push(Measured {
                name: name.clone(),
                weight: f.value.priority.weight(),
                parts,
            });
        }
    }
    out
}

pub(crate) fn role_field(mapping: &Mapping, role: FieldRole) -> Result<Option<String>, Error> {
    mapping
        .fields
        .iter()
        .find(|(_, f)| f.value.roles.contains(&role))
        .map(|(name, _)| quoted(name))
        .transpose()
}

impl Runner {
    /// The ICD of the loaded `dataset`'s cut over its `rules`.
    pub fn icd(
        &self,
        dataset: &str,
        mapping: &Mapping,
        rules: &[IcdRule],
        cut: &str,
    ) -> Result<IcdReport, Error> {
        let updated = role_field(mapping, FieldRole::Updated)?;
        let fields = measured(mapping, rules, updated.is_some());
        let cells = Cells::of(mapping, rules)?;
        if fields.is_empty() && cells.is_empty() {
            return Ok(IcdReport::default());
        }
        let plan = Plan {
            dataset,
            rules,
            cut,
            fields: &fields,
            key: entity_key(mapping)?,
            record: record_key(mapping)?,
            updated,
            segment: role_field(mapping, FieldRole::Segment)?,
            dimensions: dimensions(mapping)?,
            cells: &cells,
        };
        self.records_icd(&plan, &serious(mapping, rules))?;
        let report = self.summaries(mapping, &plan);
        if fields.is_empty() {
            self.con.execute_batch("DROP TABLE IF EXISTS \"__icd\"")?;
        }
        // Otherwise `__icd` stays for the findings (decision 0059), which
        // drop it.
        report
    }

    /// What `__icd` gives: the ICD's means, the groups and the cells.
    fn summaries(&self, mapping: &Mapping, plan: &Plan<'_>) -> Result<IcdReport, Error> {
        let mut report = if plan.fields.is_empty() {
            IcdReport::default()
        } else {
            let mut r = self.icd_of_records(plan.fields, plan.key.is_some())?;
            r.groups = self.groups(mapping, plan.dimensions.len())?;
            r
        };
        report.cells = self.cells(plan.cells)?;
        Ok(report)
    }

    /// Each record's fields' ICD and its own, as the table `__icd`: `r{i}`
    /// each rule's outcome, `f{n}` each measured field's ICD, `__k` the
    /// entity's key, `__r` the record's, `__g{j}` each dimension's group,
    /// and what the SFC cells read.
    fn records_icd(&self, plan: &Plan<'_>, serious: &str) -> Result<(), Error> {
        let Plan {
            dataset,
            rules,
            cut,
            fields,
            ..
        } = *plan;
        let cut_text = quote_text(cut).map_err(invalid)?;
        let mut columns: Vec<String> = rules
            .iter()
            .enumerate()
            .map(|(i, r)| format!("{} AS \"r{i}\"", r.sql.text))
            .collect();
        columns.push(format!(
            "{} AS \"__k\"",
            plan.key.as_deref().unwrap_or("NULL")
        ));
        columns.push(format!(
            "{} AS \"__r\"",
            plan.record.as_deref().unwrap_or("NULL")
        ));
        columns.push(format!(
            "CAST({} AS VARCHAR) AS \"__s\"",
            plan.segment.as_deref().unwrap_or("NULL")
        ));
        let a = plan.updated.as_ref().map_or("NULL".to_string(), |u| {
            format!(
                "pow(0.5, greatest(date_diff('day', {u}, CAST({cut_text} AS DATE)), 0) / 365.0)"
            )
        });
        columns.push(format!("{a} AS \"__a\""));
        columns.extend(plan.cells.read());
        let groups: Vec<String> = (0..plan.dimensions.len())
            .map(|j| format!("\"__g{j}\""))
            .collect();
        for (j, d) in plan.dimensions.iter().enumerate() {
            columns.push(format!(
                "NULLIF(trim(CAST({d} AS VARCHAR)), '') AS \"__g{j}\""
            ));
        }
        let mut kept = vec![
            "\"__k\"".to_string(),
            "\"__r\"".to_string(),
            "\"__s\"".to_string(),
        ];
        kept.extend(groups);
        kept.extend((0..rules.len()).map(|i| format!("\"r{i}\"")));
        kept.push(format!("{serious} AS \"__serious\""));
        kept.extend(
            fields
                .iter()
                .enumerate()
                .map(|(i, f)| format!("{} AS \"f{i}\"", weighted(&f.parts))),
        );
        kept.extend(plan.cells.kept());
        let record: Vec<(f64, String)> = fields
            .iter()
            .enumerate()
            .map(|(i, f)| (f64::from(f.weight), format!("\"f{i}\"")))
            .collect();
        let sql = format!(
            "CREATE OR REPLACE TEMP TABLE \"__icd\" AS
             WITH r AS (SELECT {} FROM {}),
                  f AS (SELECT {} FROM r)
             SELECT *, {} AS \"__icd\" FROM f",
            columns.join(", "),
            quoted(dataset)?,
            kept.join(", "),
            weighted(&record)
        );
        let cut_params: usize = rules.iter().map(|r| r.sql.cut_params).sum();
        let cuts = vec![cut; cut_params];
        self.con
            .execute(&sql, duckdb::params_from_iter(cuts.iter()))?;
        Ok(())
    }

    fn icd_of_records(&self, fields: &[Measured], entities: bool) -> Result<IcdReport, Error> {
        let means: Vec<String> = (0..fields.len())
            .map(|i| format!("avg(\"f{i}\")"))
            .collect();
        let (records, means): (Option<f64>, Vec<Option<f64>>) = self.con.query_row(
            &format!("SELECT avg(\"__icd\"), {} FROM \"__icd\"", means.join(", ")),
            [],
            |r| {
                let all: Result<Vec<Option<f64>>, _> =
                    (1..=fields.len()).map(|i| r.get(i)).collect();
                Ok((r.get(0)?, all?))
            },
        )?;
        let entities = if entities {
            self.entities(fields)?
        } else {
            Vec::new()
        };
        let igcd = if entities.is_empty() {
            records
        } else {
            let known: Vec<f64> = entities.iter().filter_map(|e| e.icd).collect();
            (!known.is_empty()).then(|| known.iter().sum::<f64>() / known.len() as f64)
        };
        Ok(IcdReport {
            igcd,
            fields: fields.iter().map(|f| f.name.clone()).zip(means).collect(),
            entities,
            groups: Vec::new(),
            cells: Vec::new(),
        })
    }

    fn entities(&self, fields: &[Measured]) -> Result<Vec<EntityIcd>, Error> {
        let per_field: Vec<String> = fields
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let agg = if f.weight == CRITICAL { "min" } else { "avg" };
                format!("{agg}(\"f{i}\") AS \"e{i}\"")
            })
            .collect();
        let icd: Vec<(f64, String)> = fields
            .iter()
            .enumerate()
            .map(|(i, f)| (f64::from(f.weight), format!("\"e{i}\"")))
            .collect();
        let sql = format!(
            "WITH e AS (SELECT \"__k\", mode(\"__s\") AS \"__s\", {} FROM \"__icd\"
                         WHERE \"__k\" IS NOT NULL GROUP BY \"__k\")
             SELECT \"__k\", {}, \"__s\" FROM e ORDER BY \"__k\"",
            per_field.join(", "),
            weighted(&icd)
        );
        let mut stmt = self.con.prepare(&sql)?;
        let rows = stmt.query_map([], |r| {
            Ok(EntityIcd {
                key: r.get(0)?,
                icd: r.get(1)?,
                segment: r.get(2)?,
            })
        })?;
        rows.collect()
    }
}
