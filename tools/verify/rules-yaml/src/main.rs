//! Verification for decision 0033 (step 1): which YAML library reads the
//! rules for `quality-rules`. Lint must point at the line and column of
//! what is wrong, the YAML comes from users, and codes keep leading zeros.
//! Synthetic rules only.
//!
//! ```sh
//! cargo run --release --manifest-path tools/verify/rules-yaml/Cargo.toml
//! ```

use std::time::Instant;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct RulesFile {
    dataset: String,
    #[serde(default)]
    lists: std::collections::BTreeMap<String, Vec<String>>,
    rules: Vec<Rule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct Rule {
    id: String,
    field: String,
    check: String,
    min: Option<f64>,
    named: Option<String>,
}

/// The same rule, with the position of each value (serde-saphyr only).
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SpannedRule {
    id: serde_saphyr::Spanned<String>,
    field: serde_saphyr::Spanned<String>,
}

const VALID: &str = "dataset: clients
lists:
  answers: [SI, NO, yes, off]
  codes: [08, 0123, 1e3, 007]
rules:
  - id: R-1
    field: Celular
    check: format
    named: mobile_co
";
const WRONG_TYPE: &str = "dataset: clients
rules:
  - id: R-1
    field: Edad
    check: range
    min: diez
";
const UNKNOWN_KEY: &str = "dataset: clients
rules:
  - id: R-1
    feild: Edad
    check: required
";
const DUPLICATE_KEY: &str = "dataset: clients
rules:
  - id: R-1
    field: Edad
    check: required
    check: range
";

fn report(name: &str, outcome: Result<String, String>) {
    match outcome {
        Ok(s) => println!("[OK  ] {name}: {s}"),
        Err(s) => println!("[FAIL] {name}: {s}"),
    }
}

/// The error must name line 6 (or the given line) of the document.
fn expect_line<E: std::fmt::Display>(
    r: Result<RulesFile, E>,
    line: usize,
    pos: Option<(u64, u64)>,
) -> Result<String, String> {
    match r {
        Ok(_) => Err("accepted".into()),
        Err(e) => {
            let text = e.to_string().replace('\n', " | ");
            let at = pos
                .map(|(l, c)| format!(" [location {l}:{c}]"))
                .unwrap_or_default();
            let names_line = pos.map(|(l, _)| l == line as u64).unwrap_or(false)
                || text.contains(&format!("line {line}"))
                || text.contains(&format!(":{line}:"));
            let msg = format!("{}{at}", &text[..text.len().min(160)]);
            if names_line {
                Ok(msg)
            } else {
                Err(format!("no line {line}: {msg}"))
            }
        }
    }
}

/// An alias bomb: each level doubles the one before (2^30 strings).
fn bomb() -> String {
    let mut s = String::from("a0: &a0 [x, x]\n");
    for i in 1..30 {
        s.push_str(&format!("a{i}: &a{i} [*a{p}, *a{p}]\n", p = i - 1));
    }
    s
}

fn saphyr_pos(e: &serde_saphyr::Error) -> Option<(u64, u64)> {
    e.location().map(|l| (l.line(), l.column()))
}

fn main() {
    println!("== serde-saphyr 2.0.0");
    report(
        "valid file",
        serde_saphyr::from_str::<RulesFile>(VALID)
            .map(|f| format!("{} rule(s), lists {:?}", f.rules.len(), f.lists))
            .map_err(|e| e.to_string()),
    );
    report(
        "wrong type at 6",
        expect_line(
            serde_saphyr::from_str::<RulesFile>(WRONG_TYPE)
                .map_err(|e| {
                    let p = saphyr_pos(&e);
                    (e, p)
                })
                .map_err(|(e, _)| e),
            6,
            serde_saphyr::from_str::<RulesFile>(WRONG_TYPE)
                .err()
                .as_ref()
                .and_then(saphyr_pos),
        ),
    );
    report(
        "unknown key at 4",
        expect_line(
            serde_saphyr::from_str::<RulesFile>(UNKNOWN_KEY),
            4,
            serde_saphyr::from_str::<RulesFile>(UNKNOWN_KEY)
                .err()
                .as_ref()
                .and_then(saphyr_pos),
        ),
    );
    report(
        "duplicate key at 6",
        expect_line(
            serde_saphyr::from_str::<RulesFile>(DUPLICATE_KEY),
            6,
            serde_saphyr::from_str::<RulesFile>(DUPLICATE_KEY)
                .err()
                .as_ref()
                .and_then(saphyr_pos),
        ),
    );
    report(
        "spans of valid values",
        serde_saphyr::from_str::<Vec<SpannedRule>>("- id: R-1\n  field: Celular\n")
            .map(|r| {
                format!(
                    "field at {}:{}",
                    r[0].field.referenced.line(),
                    r[0].field.referenced.column()
                )
            })
            .map_err(|e| e.to_string()),
    );
    let t = Instant::now();
    let b =
        serde_saphyr::from_str::<std::collections::BTreeMap<String, serde_yaml_ng::Value>>(&bomb());
    report(
        "alias bomb refused",
        match b {
            Ok(_) => Err(format!("accepted in {:?}", t.elapsed())),
            Err(e) => Ok(format!(
                "{:.100} ({:?})",
                e.to_string().replace('\n', " "),
                t.elapsed()
            )),
        },
    );

    println!("== serde_yaml_ng 0.10.0");
    let ng_pos =
        |e: &serde_yaml_ng::Error| e.location().map(|l| (l.line() as u64, l.column() as u64));
    report(
        "valid file",
        serde_yaml_ng::from_str::<RulesFile>(VALID)
            .map(|f| format!("{} rule(s), lists {:?}", f.rules.len(), f.lists))
            .map_err(|e| e.to_string()),
    );
    report(
        "wrong type at 6",
        expect_line(
            serde_yaml_ng::from_str::<RulesFile>(WRONG_TYPE),
            6,
            serde_yaml_ng::from_str::<RulesFile>(WRONG_TYPE)
                .err()
                .as_ref()
                .and_then(ng_pos),
        ),
    );
    report(
        "unknown key at 4",
        expect_line(
            serde_yaml_ng::from_str::<RulesFile>(UNKNOWN_KEY),
            4,
            serde_yaml_ng::from_str::<RulesFile>(UNKNOWN_KEY)
                .err()
                .as_ref()
                .and_then(ng_pos),
        ),
    );
    report(
        "duplicate key at 6",
        expect_line(
            serde_yaml_ng::from_str::<RulesFile>(DUPLICATE_KEY),
            6,
            serde_yaml_ng::from_str::<RulesFile>(DUPLICATE_KEY)
                .err()
                .as_ref()
                .and_then(ng_pos),
        ),
    );
    println!("[----] spans of valid values: no Spanned type");
    let t = Instant::now();
    let b = serde_yaml_ng::from_str::<std::collections::BTreeMap<String, serde_yaml_ng::Value>>(
        &bomb(),
    );
    report(
        "alias bomb refused",
        match b {
            Ok(_) => Err(format!("accepted in {:?}", t.elapsed())),
            Err(e) => Ok(format!("{:.100} ({:?})", e.to_string(), t.elapsed())),
        },
    );
}
