//! The engine comparison (decision 0090, point 11; run in CI on test data,
//! decision 0091 point 5): two reports of one cut, from DuckDB and from
//! Trino, compared field by field. Counts, names and keys must be equal;
//! doubles agree within a relative 1e-12 (absolute 1e-15 near zero, point
//! 8.3: parallel summation, not approximation). What it gives is the kind
//! of each mismatch and how many: never a value, a key or a hash.

use std::collections::BTreeMap;

use crate::report::Report;

/// Relative tolerance of a double.
const RELATIVE: f64 = 1e-12;
/// Absolute tolerance near zero.
const ABSOLUTE: f64 = 1e-15;

/// One kind of mismatch and how many there were.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub kind: &'static str,
    pub count: u64,
}

/// Whether two doubles agree within the tolerance; two NaN agree.
pub fn close(a: f64, b: f64) -> bool {
    if a.is_nan() || b.is_nan() {
        return a.is_nan() && b.is_nan();
    }
    if a == b {
        return true;
    }
    let d = (a - b).abs();
    d <= ABSOLUTE || d <= RELATIVE * a.abs().max(b.abs())
}

fn close_opt(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => close(a, b),
        (None, None) => true,
        _ => false,
    }
}

#[derive(Default)]
struct Tally(BTreeMap<&'static str, u64>);

impl Tally {
    fn check(&mut self, kind: &'static str, same: bool) {
        if !same {
            *self.0.entry(kind).or_default() += 1;
        }
    }

    /// Rows keyed by `key`: a key on one side only, or twice on one side,
    /// is a `<kind>` mismatch; rows on both sides are compared by `same`.
    fn rows<T, K: Ord>(
        &mut self,
        kind: &'static str,
        a: &[T],
        b: &[T],
        key: impl Fn(&T) -> K,
        mut same: impl FnMut(&mut Self, &T, &T),
    ) {
        let left: BTreeMap<K, &T> = a.iter().map(|r| (key(r), r)).collect();
        let right: BTreeMap<K, &T> = b.iter().map(|r| (key(r), r)).collect();
        self.check(kind, left.len() == a.len() && right.len() == b.len());
        for (k, l) in &left {
            match right.get(k) {
                Some(r) => same(self, l, r),
                None => self.check(kind, false),
            }
        }
        for k in right.keys() {
            self.check(kind, left.contains_key(k));
        }
    }
}

/// The mismatches between `duckdb`'s and `trino`'s reports of one cut, by
/// kind, in name order; none when they agree. Durations, stages, snapshot
/// ids and the compiled SQL are each engine's own and are not compared.
pub fn compare(duckdb: &Report, trino: &Report) -> Vec<Mismatch> {
    let mut t = Tally::default();
    t.check("cut", duckdb.cut == trino.cut);
    t.check("records", duckdb.records == trino.records);
    t.check("rules_commit", duckdb.rules_commit == trino.rules_commit);
    t.check("igcd", close_opt(duckdb.igcd, trino.igcd));
    t.rows(
        "rules",
        &duckdb.rules,
        &trino.rules,
        |r| r.rule.clone(),
        |t, a, b| {
            t.check(
                "rules.counts",
                a.applies == b.applies && a.failures == b.failures,
            );
        },
    );
    t.rows(
        "fields",
        &duckdb.fields,
        &trino.fields,
        |f| f.field.clone(),
        |t, a, b| {
            t.check(
                "fields.profile",
                (a.empty, a.distinct, &a.pattern, a.pattern_count)
                    == (b.empty, b.distinct, &b.pattern, b.pattern_count),
            );
            t.check("fields.icd", close_opt(a.icd, b.icd));
            t.check("fields.recency", a.recency == b.recency);
        },
    );
    t.rows(
        "entities",
        &duckdb.entities,
        &trino.entities,
        |e| e.0.clone(),
        |t, a, b| {
            t.check("entities.icd", close_opt(a.1, b.1));
            t.check("entities.segment", a.2 == b.2);
        },
    );
    groups_and_after(&mut t, duckdb, trino);
    t.0.into_iter()
        .map(|(kind, count)| Mismatch { kind, count })
        .collect()
}

/// The groups, distributions, outliers, cells and queues.
fn groups_and_after(t: &mut Tally, duckdb: &Report, trino: &Report) {
    t.rows(
        "groups",
        &duckdb.groups,
        &trino.groups,
        |g| (g.dimension.clone(), g.group.clone()),
        |t, a, b| {
            t.check(
                "groups.counts",
                (a.records, a.serious) == (b.records, b.serious),
            );
            t.check("groups.icd", close_opt(a.icd, b.icd));
        },
    );
    t.rows(
        "distributions",
        &duckdb.distributions,
        &trino.distributions,
        |d| d.variable.clone(),
        |t, a, b| {
            let same = a.percentiles.len() == b.percentiles.len()
                && a.percentiles
                    .iter()
                    .zip(&b.percentiles)
                    .all(|(x, y)| close(*x, *y));
            t.check("distributions.percentiles", same);
        },
    );
    t.rows(
        "outliers",
        &duckdb.outliers,
        &trino.outliers,
        |o| (o.variable.clone(), o.dimension.clone(), o.group.clone()),
        |t, a, b| t.check("outliers.counts", a == b),
    );
    t.rows(
        "cells",
        &duckdb.cells,
        &trino.cells,
        |c| c.field.clone(),
        |t, a, b| {
            t.check("cells.counts", a == b);
        },
    );
    t.rows(
        "queues",
        &duckdb.queues,
        &trino.queues,
        |q| (q.dimension.clone(), q.group.clone(), q.since.clone()),
        |t, a, b| {
            t.check(
                "queues.counts",
                (a.open, a.resolved) == (b.open, b.resolved),
            );
            t.check("queues.priority", close(a.priority, b.priority));
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A synthetic report: invented rules, groups and an entity hash.
    fn report() -> Report {
        serde_json::from_value(json!({
            "cut": "2026-10-31", "records": 3, "duration_ms": 10, "rules_commit": "c1",
            "rules": [{"rule": "R1", "applies": 3, "failures": 1}],
            "igcd": 0.75,
            "entities": [["ab".repeat(32), 0.5, "PN"]],
            "groups": [{"dimension": "Office", "group": "O-1", "records": 3, "serious": 1, "icd": 0.7}],
            "cells": [{"field": "Phone", "ok": 2, "total": 3}],
            "queues": [{"dimension": "Office", "group": "O-1", "since": "2026-10-31",
                        "open": 1, "resolved": 0, "priority": 0.25}]
        }))
        .unwrap()
    }

    #[test]
    fn equal_reports_and_engine_own_fields_give_no_mismatch() {
        let mut trino = report();
        trino.duration_ms = 99;
        trino.compiler = "trino".into();
        trino.igcd = Some(0.75 * (1.0 + 1e-14));
        assert_eq!(compare(&report(), &trino), []);
    }

    #[test]
    fn a_mismatch_is_its_kind_and_count_never_a_value_or_key() {
        let mut trino = report();
        trino.rules[0].failures = 2;
        trino.igcd = Some(0.76);
        trino.entities[0].0 = "cd".repeat(32);
        trino.queues[0].priority = 0.25 + 1e-9;
        let seen = compare(&report(), &trino);
        let kinds: Vec<(&str, u64)> = seen.iter().map(|m| (m.kind, m.count)).collect();
        assert_eq!(
            kinds,
            [
                ("entities", 2),
                ("igcd", 1),
                ("queues.priority", 1),
                ("rules.counts", 1)
            ]
        );
        let text = format!("{seen:?}");
        for leaked in ["abab", "cdcd", "R1", "0.76", "O-1"] {
            assert!(!text.contains(leaked), "{leaked} in {text}");
        }
    }

    #[test]
    fn doubles_agree_within_the_tolerance_and_nan_with_nan() {
        assert!(close(1.0, 1.0 + 1e-13));
        assert!(!close(1.0, 1.0 + 1e-11));
        assert!(close(0.0, 1e-16));
        assert!(close(f64::NAN, f64::NAN));
        assert!(!close(f64::NAN, 0.0));
    }
}
