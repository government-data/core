//! The cases run on an embedded DuckDB 2.0.0: each compiled rule gives the
//! expected three-valued result on each synthetic row.

mod cases;

use duckdb::types::Value;
use duckdb::Connection;
use quality_rules_sql::Engine;

#[test]
fn every_case_gives_its_results_on_duckdb() {
    let con = Connection::open_in_memory().unwrap();
    let mut failures = Vec::new();
    for case in cases::cases() {
        let (sql, cut_params) = cases::query(&case, Engine::DuckDb);
        let mut stmt = con
            .prepare(&sql)
            .unwrap_or_else(|e| panic!("{}: {e}\n{sql}", case.name));
        let cut = vec![cases::CUT; cut_params];
        let width = case.expected[0].len();
        let got: Vec<Vec<Option<bool>>> = stmt
            .query_map(duckdb::params_from_iter(cut.iter()), |row| {
                (0..width)
                    .map(|i| {
                        row.get::<_, Value>(i).map(|v| match v {
                            Value::Boolean(b) => Some(b),
                            _ => None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_else(|e| panic!("{}: {e}\n{sql}", case.name))
            .collect::<Result<_, _>>()
            .unwrap();
        let expected: Vec<Vec<Option<bool>>> = case.expected.iter().map(|r| r.to_vec()).collect();
        if got != expected {
            failures.push(format!(
                "{}\n  expected {expected:?}\n  got      {got:?}\n  {sql}",
                case.name
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
