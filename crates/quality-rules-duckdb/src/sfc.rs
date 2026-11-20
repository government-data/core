//! The SFC indicator's cells (decision 0058): per field holding the `sfc`
//! role, the active records and those where it is right, filled and failing
//! no rule that reads it. Over the rules' results in the ICD's records
//! (`__icd`), never a second evaluator; emptiness and the active test are
//! written by `quality-rules-sql` from the rule language's tree.

use quality::mapping::{FieldRole, Mapping};
use quality::worker_api::SfcCell;
use quality_rules_sql::{Engine, Tables};
use quality_rules_tree::{Expr, Function, Literal};

use crate::icd::IcdRule;
use crate::{invalid, Error, Runner};

/// What the records' query reads for the cells, and what it keeps.
#[derive(Debug, Default)]
pub(crate) struct Cells {
    names: Vec<String>,
    /// Per cell, the SQL of its field being filled, over the dataset.
    filled: Vec<String>,
    /// Per cell, the rules that read it, by index.
    rules: Vec<Vec<usize>>,
    /// The SQL of a record being active, over the dataset.
    active: String,
}

fn sql(e: &Expr) -> Result<String, Error> {
    let sql = quality_rules_sql::expr(e, Engine::DuckDb, &Tables::new()).map_err(invalid)?;
    Ok(sql.text)
}

fn call(function: Function, x: Expr) -> Expr {
    Expr::Call {
        function,
        args: vec![x],
    }
}

/// Trimmed of spaces and U+00A0 and in upper case, as the active test reads.
fn active_value(v: &str) -> String {
    v.trim_matches([' ', '\u{a0}']).to_uppercase()
}

impl Cells {
    pub(crate) fn of(mapping: &Mapping, rules: &[IcdRule]) -> Result<Self, Error> {
        let mut cells = Self {
            active: "TRUE".to_string(),
            ..Self::default()
        };
        for (name, f) in &mapping.fields {
            let field = || Expr::Field(name.clone());
            if f.value.roles.contains(&FieldRole::Active) {
                cells.active = sql(&Expr::InList {
                    expr: Box::new(call(Function::Upper, call(Function::Trim, field()))),
                    list: f
                        .value
                        .active_values
                        .iter()
                        .map(|v| Expr::Literal(Literal::Text(active_value(v))))
                        .collect(),
                    negated: false,
                })?;
            }
            if f.value.roles.contains(&FieldRole::Sfc) {
                cells.names.push(name.clone());
                cells
                    .filled
                    .push(sql(&Expr::Not(Box::new(call(Function::IsEmpty, field()))))?);
                cells.rules.push(
                    (0..rules.len())
                        .filter(|&i| rules[i].reads.iter().any(|r| r == name))
                        .collect(),
                );
            }
        }
        Ok(cells)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The columns the records' query adds over the dataset: each cell
    /// filled, and the record active.
    pub(crate) fn read(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .filled
            .iter()
            .enumerate()
            .map(|(j, f)| format!("{f} AS \"__n{j}\""))
            .collect();
        out.push(format!("COALESCE({}, FALSE) AS \"__act\"", self.active));
        out
    }

    /// The columns the records' table keeps: each cell right (filled and
    /// failing no rule that reads it; one that does not apply leaves it
    /// right), and the record active.
    pub(crate) fn kept(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .rules
            .iter()
            .enumerate()
            .map(|(j, rules)| {
                let fails: Vec<String> = rules
                    .iter()
                    .map(|i| format!("COALESCE(\"r{i}\", FALSE)"))
                    .collect();
                if fails.is_empty() {
                    format!("\"__n{j}\" AS \"__c{j}\"")
                } else {
                    format!(
                        "(\"__n{j}\" AND NOT ({})) AS \"__c{j}\"",
                        fails.join(" OR ")
                    )
                }
            })
            .collect();
        out.push("\"__act\"".to_string());
        out
    }
}

impl Runner {
    /// Each cell's active records and right ones, over `__icd`.
    pub(crate) fn cells(&self, cells: &Cells) -> Result<Vec<SfcCell>, Error> {
        if cells.is_empty() {
            return Ok(Vec::new());
        }
        let ok: Vec<String> = (0..cells.names.len())
            .map(|j| format!("count(*) FILTER (WHERE \"__act\" AND \"__c{j}\")"))
            .collect();
        let sql = format!(
            "SELECT count(*) FILTER (WHERE \"__act\"), {} FROM \"__icd\"",
            ok.join(", ")
        );
        self.con.query_row(&sql, [], |r| {
            let total: i64 = r.get(0)?;
            cells
                .names
                .iter()
                .enumerate()
                .map(|(j, field)| {
                    Ok(SfcCell {
                        field: field.clone(),
                        ok: r.get(j + 1)?,
                        total,
                    })
                })
                .collect()
        })
    }
}
