//! Writes the semantic cases as Trino statements, one per line:
//! `name<TAB>statement<TAB>expected`, where the statement binds the cut and
//! `expected` is the rows' results as JSON. tools/verify/rules_sql_equivalence.sh
//! runs them on the dev cluster; the same cases run on DuckDB in `cargo test`.

#[path = "../tests/cases/mod.rs"]
mod cases;

use quality_rules_sql::{quote_text, Engine};

fn main() {
    for case in cases::cases() {
        let (sql, cut_params) = cases::query(&case, Engine::Trino);
        let using = vec![format!("DATE '{}'", cases::CUT); cut_params].join(", ");
        let statement = if cut_params == 0 {
            sql
        } else {
            format!(
                "EXECUTE IMMEDIATE {} USING {using}",
                quote_text(&sql).expect("no control characters")
            )
        };
        let expected: Vec<String> = case
            .expected
            .iter()
            .map(|row| {
                let cells: Vec<&str> = row
                    .iter()
                    .map(|v| v.map_or("null", |b| if b { "true" } else { "false" }))
                    .collect();
                format!("[{}]", cells.join(","))
            })
            .collect();
        println!("{}\t{statement}\t[{}]", case.name, expected.join(","));
    }
}
